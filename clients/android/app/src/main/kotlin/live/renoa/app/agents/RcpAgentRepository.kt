package live.renoa.app.agents

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.emitAll
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withTimeoutOrNull
import live.renoa.rcp.DeviceKeyRequired
import live.renoa.rcp.PendingTextCommand
import live.renoa.rcp.RcpErrorCode
import live.renoa.rcp.RcpException
import live.renoa.rcp.RcpState
import live.renoa.rcp.RcpSurface
import live.renoa.rcp.TargetSummary
import java.time.ZoneId
import kotlin.time.Duration
import kotlin.time.Duration.Companion.minutes
import kotlin.time.Duration.Companion.seconds
import kotlin.time.TimeSource
import kotlin.uuid.Uuid

/**
 * The screens' Renoa over RCP. The agents are the targets the owner's nodes advertise; each has
 * one chat task ([chatTaskId]), followed with its live text while the app runs.
 *
 * [state] is the outbox [surface] writes to. A message the coordinator refused is dropped; any other
 * failure leaves it there, and it is sent again with the same identity after the next connect and
 * on every roster check, as [UnsentMessages] decides. A message it gives up stays in the chat marked
 * [ChatMessage.notSent] until [retry]. A message the outbox held when the chat opened shows until it
 * arrives.
 *
 * Making an agent, reading its profile and the routines are Host control, carried by the same
 * connection ([HostAgents], [HostRoutines]). The routines are read again on every roster check, and
 * each Host agent's purpose until the Host has answered for it.
 * A new agent is an `agent:<id>` target once its node next advertises; [create] waits for that and
 * follows its chat. Abilities and edits are not on the Host's agent API yet, so those calls are
 * refused as [Problem.NotYet] and nothing is sent.
 */
class RcpAgentRepository(
    private val surface: RcpSurface,
    private val state: RcpState,
    private val scope: CoroutineScope,
    private val retryAfter: (attempt: Int) -> Duration = { attempt -> (1L shl minOf(attempt, 5)).seconds },
    private val rosterEvery: Duration = 1.minutes,
    private val here: () -> ZoneId = { ZoneId.systemDefault() },
    private val clock: TimeSource = TimeSource.Monotonic,
) : AgentRepository {
    private class Room(val target: TargetSummary, val taskId: Uuid) {
        val journal = ChatJournal()
        val conversation = MutableStateFlow(Conversation())

        /** Set once [RcpSurface.attach] returned; from then on [RcpSurface.connect] re-attaches it. */
        var attached = false

        /** What the agent is for, as the Host keeps it; null until read. */
        var purpose: String? = null
    }

    private val lock = Any()
    private val rooms = LinkedHashMap<String, Room>()
    private val roster = MutableStateFlow<List<AgentSummary>>(emptyList())
    private val unsent =
        UnsentMessages(surface, state, clock) { taskId, change ->
            synchronized(lock) { rooms.values.firstOrNull { it.taskId == taskId } }?.let { update(it, change) }
        }
    private val hostClient = HostClient(surface)
    private val host = HostAgents(hostClient)
    private val hostRoutines = HostRoutines(hostClient)
    private val owner = HostOwner(hostClient)

    /** The automations as the Host last listed them, by id, with the revision a change starts from. */
    private val automations = LinkedHashMap<String, ObservedAutomation>()
    private val routineList = MutableStateFlow<List<Routine>>(emptyList())

    /** One [follow] at a time, so the roster check and a new agent never open the same chat twice. */
    private val following = Mutex()

    private val linkState = MutableStateFlow(Link.Connecting)

    override val link: StateFlow<Link> = linkState.asStateFlow()

    override val agents: StateFlow<List<AgentSummary>> = roster.asStateFlow()

    override val briefing: StateFlow<Briefing?> = MutableStateFlow<Briefing?>(null).asStateFlow()

    override val routines: StateFlow<List<Routine>> = routineList.asStateFlow()

    /**
     * Connects, follows every agent's chat, and reconnects after a drop, until [scope] is cancelled
     * or Renoa refuses this phone's sign-in, which no retry can fix.
     */
    fun start() {
        scope.launch {
            var attempt = 0
            var refusals = 0
            while (true) {
                try {
                    surface.connect()
                    unsent.send()
                    follow(surface.listTargets())
                    linkState.value = Link.Connected
                    refusals = 0
                    readRoutines()
                    readPurposes()
                    // Only a connection that got this far resets the wait, so one that fails at once backs off.
                    attempt = 0
                    coroutineScope {
                        // An agent made elsewhere joins, and a message the node could not take yet goes, without a reconnect.
                        val roll =
                            launch {
                                while (true) {
                                    delay(rosterEvery)
                                    unsent.send()
                                    follow(surface.listTargets())
                                    readRoutines()
                                    readPurposes()
                                }
                            }
                        surface.awaitDisconnect()
                        roll.cancel()
                    }
                } catch (cancelled: CancellationException) {
                    throw cancelled
                } catch (failure: Exception) {
                    // Once could be a glitch in the handshake; refused twice running, the sign-in is gone.
                    refusals = if (failure.signsOut) refusals + 1 else 0
                    if (refusals >= SIGN_OUT_AFTER) {
                        linkState.value = Link.SignedOut
                        surface.close()
                        return@launch
                    }
                    // A connection that failed partway is closed before the next attempt.
                    runCatching { surface.disconnect() }
                }
                linkState.value = Link.Lost
                // Live text is not replayed, so a draft cut by the drop would be glued to text from after it.
                synchronized(lock) { rooms.values.toList() }.forEach { update(it) { connectionLost() } }
                delay(retryAfter(attempt++))
            }
        }
    }

    /**
     * Opens and attaches the chat of each agent not attached yet; [RcpSurface.connect] re-attaches the
     * rest. A room outlives a failed attach, because the client keeps the cursor of what it applied
     * and replays only what follows. One agent that cannot be opened does not hold up the others.
     */
    private suspend fun follow(targets: List<TargetSummary>) = following.withLock {
        for (target in targets) {
            val room = synchronized(lock) { rooms.getOrPut(agentId(target)) { Room(target, chatTaskId(target.nodeId, target.target)) } }
            if (room.attached) continue
            // Sent before the app last closed: shown until its record arrives. Read once the room exists,
            // so a refusal from here on reaches the room and outlasts this read.
            val unsent = state.pendingCommands().filter { it.taskId == room.taskId }
            if (unsent.isNotEmpty()) update(room) { unsent.forEach { sending(it.commandId, it.text) } }
            try {
                surface.openTask(room.taskId, target.nodeId, target.target)
                surface.attach(room.taskId, live = { commandId, activity -> update(room) { live(commandId, activity) } }) { event ->
                    update(room) { apply(event) }
                }
            } catch (refused: RcpException) {
                if (refused.requestId == null) throw refused
                continue
            }
            synchronized(lock) { room.attached = true }
            publish()
        }
    }

    private fun update(room: Room, change: ChatJournal.() -> Unit) {
        synchronized(lock) {
            room.journal.change()
            room.conversation.value = room.journal.conversation
        }
        publish()
    }

    /**
     * Lists the attached agents, and names each routine's agent from them. It writes under the lock,
     * so an older roster never overwrites a newer one.
     */
    private fun publish() {
        synchronized(lock) {
            roster.value =
                rooms.filterValues { it.attached }.map { (id, room) ->
                    AgentSummary(id, room.target.name, purpose = room.purpose.orEmpty(), state = room.journal.state, glance = room.journal.glance)
                }
            val zone = here()
            routineList.value =
                automations.values.map { automation ->
                    val agent = rooms.entries.firstOrNull { (_, room) -> room.attached && room.target.target == "$AGENT_TARGET${automation.agentId}" }
                    automation.routine(agent?.key, zone)
                }
        }
    }

    /**
     * Reads the routines again. While the Host cannot be read, the last ones stay. The Host answers
     * calls in any order, so a routine the app already holds at a newer revision keeps it.
     */
    private suspend fun readRoutines() {
        val listed =
            try {
                hostRoutines.observe()
            } catch (failed: RepositoryException) {
                return
            }
        synchronized(lock) {
            val held = automations.toMap()
            automations.clear()
            listed.forEach { read -> automations[read.id] = held[read.id]?.takeIf { it.revision > read.revision } ?: read }
        }
        publish()
    }

    /**
     * Reads what each followed Host agent is for. Once the Host answered, with the definition or with
     * no such agent, it is not asked again this launch; any other failure is asked again on the next
     * roster check.
     */
    private suspend fun readPurposes() {
        val unread = synchronized(lock) { rooms.values.filter { it.attached && it.purpose == null } }
        for (room in unread) {
            val hostId = hostAgentId(agentId(room.target)) ?: continue
            val purpose =
                try {
                    host.definition(hostId).operational.purpose.orEmpty()
                } catch (failed: RepositoryException) {
                    if (failed.problem != Problem.NotFound) continue
                    ""
                }
            synchronized(lock) { room.purpose = purpose }
            publish()
        }
    }

    override suspend fun hasAgents(): Boolean = owner.hasAgents()

    override suspend fun ownerProfile(): OwnerProfile = owner.profile()

    override suspend fun saveOwnerProfile(expectedRevision: String, content: String): OwnerProfile = owner.saveProfile(expectedRevision, content)

    override suspend fun setRoutine(routineId: String, on: Boolean, operationId: String) {
        val automation = synchronized(lock) { automations[routineId] } ?: throw RepositoryException(Problem.NotFound, "no routine $routineId")
        if (automation.enabled == on) return
        val changed =
            try {
                hostRoutines.setEnabled(automation, on, operationId)
            } catch (refused: RepositoryException) {
                // A conflict means the routine changed elsewhere; read it again so the next change starts from there.
                if (refused.problem == Problem.Conflict || refused.problem == Problem.NotFound) readRoutines()
                throw refused
            }
        synchronized(lock) {
            // A read that answered meanwhile may have dropped the routine or seen a newer revision; either stands.
            val held = automations[routineId]
            if (held != null && held.revision < changed.revision) automations[routineId] = changed
        }
        publish()
    }

    private fun room(agentId: String): Room =
        synchronized(lock) { rooms[agentId] } ?: throw RepositoryException(Problem.NotFound, "no agent $agentId")

    /** Waits for the agent to join the roster, as after a restart that reopened its chat before the first connect. */
    override fun conversation(agentId: String): Flow<Conversation> =
        flow {
            roster.first { agents -> agents.any { it.id == agentId } }
            emitAll(room(agentId).conversation)
        }

    /**
     * Returns once the message is in the outbox and shown. A refusal takes it back out and throws;
     * any other failure leaves it to be sent again.
     */
    override suspend fun send(agentId: String, text: String) {
        val room = room(agentId)
        if (text.isBlank()) throw RepositoryException(Problem.Invalid, "A message needs some words in it.")
        val submission =
            try {
                surface.submitText(room.taskId, text.trim())
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (offline: IllegalStateException) {
                // Nothing reached the outbox: the client refuses a message while it is not connected,
                // and the outbox refuses one it cannot keep.
                throw RepositoryException(Problem.Unavailable, offline.message.orEmpty())
            } catch (tooLong: IllegalArgumentException) {
                throw RepositoryException(Problem.Invalid, tooLong.message.orEmpty())
            }
        update(room) { sending(submission.commandId, text.trim()) }
        try {
            submission.accepted.await()
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (failure: Exception) {
            if (failure !is RcpException || !failure.refusesTheRequest) {
                // Answered but not taken: the wait to give it up starts now.
                if (failure is RcpException && failure.requestId != null) unsent.notTaken(submission.commandId)
                return
            }
            state.removeCommand(submission.commandId)
            update(room) { refused(submission.commandId) }
            throw failure.asProblem()
        }
    }

    /**
     * Puts the message back in the outbox under its own identity, shows it as sending again, and sends
     * its chat's unsent messages now. One that cannot go now waits, as any unsent message does.
     */
    override suspend fun retry(agentId: String, messageId: String) {
        val room = room(agentId)
        val commandId = Uuid.parseOrNull(messageId) ?: return
        var text: String? = null
        update(room) { text = resend(commandId) }
        val words = text ?: return
        try {
            state.enqueueCommand(PendingTextCommand(commandId, room.taskId, words))
        } catch (unkept: IllegalStateException) {
            update(room) { notSent(commandId) }
            throw RepositoryException(Problem.Unavailable, unkept.message.orEmpty())
        }
        try {
            unsent.send(room.taskId)
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (lost: Exception) {
            // The connection is down; the message goes after the next connect.
        }
    }

    override suspend fun answer(agentId: String, choice: String) = send(agentId, choice)

    override suspend fun markSeen(agentId: String) {
        val room = room(agentId)
        synchronized(lock) { room.journal.markSeen() }
        publish()
    }

    override suspend fun stop(agentId: String) {
        val room = room(agentId)
        val running = synchronized(lock) { room.journal.running } ?: return
        try {
            surface.stopCommand(room.taskId, running)
        } catch (refused: RcpException) {
            throw refused.asProblem()
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (lost: Exception) {
            throw RepositoryException(Problem.Unavailable, lost.message.orEmpty())
        }
    }

    override suspend fun profile(agentId: String): AgentProfile {
        val hostId = hostAgentId(agentId) ?: throw RepositoryException(Problem.NotFound, "$agentId is not an agent on the Host")
        return host.definition(hostId).profile(agentId)
    }

    /** The Host's agent API does not list abilities yet, so a new agent starts with none shown. */
    override suspend fun starterAbilities(): List<AbilitySwitch> = emptyList()

    /**
     * Makes the agent on the Host, waits up to [JOIN_WITHIN] for a node to advertise it, and returns
     * once its chat is followed. Any failure after the Host call leaves an agent that exists; the
     * same request under the same [CreateAgent.operationId] returns it again.
     */
    override suspend fun create(request: CreateAgent): AgentProfile {
        if (request.litAbilities.isNotEmpty()) throw RepositoryException(Problem.Invalid, "the Host cannot switch abilities yet")
        val record = host.create(request.operationId, request.name, request.purpose.trim().ifEmpty { null })
        val target = "$AGENT_TARGET${record.id}"
        val joined =
            try {
                withTimeoutOrNull(JOIN_WITHIN) { advertised(target) }?.also { follow(listOf(it)) }
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (refused: RcpException) {
                throw refused.asProblem()
            } catch (lost: Exception) {
                throw RepositoryException(Problem.Unavailable, lost.message.orEmpty())
            } ?: throw RepositoryException(Problem.Unavailable, "agent ${record.id} was made, but no node has offered it yet")
        // follow passes over a chat it could not open, as when the node dropped after advertising it.
        if (synchronized(lock) { rooms[agentId(joined)]?.attached } != true) {
            throw RepositoryException(Problem.Unavailable, "agent ${record.id} was made, but its chat could not be opened")
        }
        synchronized(lock) { rooms[agentId(joined)]?.purpose = record.operational.purpose.orEmpty() }
        publish()
        return record.profile(agentId(joined))
    }

    /** Checks the targets every [JOIN_EVERY] until one is [target]. */
    private suspend fun advertised(target: String): TargetSummary {
        while (true) {
            surface.listTargets().firstOrNull { it.target == target }?.let { return it }
            delay(JOIN_EVERY)
        }
    }

    override suspend fun edit(agentId: String, edit: EditProfile): AgentProfile = notYet()

    override suspend fun setAbility(agentId: String, abilityId: String, lit: Boolean): AbilitySwitch = notYet()

    private fun notYet(): Nothing = throw RepositoryException(Problem.NotYet, "the Host cannot change this from the app yet")

    private fun agentId(target: TargetSummary) = "${target.nodeId}/${target.target}"

    private companion object {
        /** How a node names the target of a Host agent. */
        const val AGENT_TARGET = "agent:"
        val JOIN_WITHIN = 30.seconds
        val JOIN_EVERY = 1.seconds

        /** The Host's id for the agent behind [agentId], or null when it names no Host agent. */
        fun hostAgentId(agentId: String): Uuid? {
            val target = agentId.substringAfter('/', "")
            if (!target.startsWith(AGENT_TARGET)) return null
            return Uuid.parseOrNull(target.removePrefix(AGENT_TARGET))
        }

        fun AgentRecord.profile(agentId: String) = AgentProfile(agentId, name, operational.purpose.orEmpty(), abilities = emptyList())
    }
}

/** What a coordinator refusal means to the screens. */
internal fun RcpException.asProblem(): RepositoryException {
    val problem =
        when (code) {
            RcpErrorCode.NotFound -> Problem.NotFound
            RcpErrorCode.Conflict -> Problem.Conflict
            RcpErrorCode.InvalidMessage -> Problem.Invalid
            else -> Problem.Unavailable
        }
    return RepositoryException(problem, message.orEmpty())
}

/** Refused sign-ins in a row after which the phone counts as signed out. */
private const val SIGN_OUT_AFTER = 2

/**
 * A refused sign-in: the handshake's refusal of the device credential (it carries no request id),
 * or a device key the coordinator wants and the phone no longer has.
 */
private val Exception.signsOut: Boolean
    get() = this is DeviceKeyRequired || (this is RcpException && code == RcpErrorCode.AuthenticationFailed && requestId == null)
