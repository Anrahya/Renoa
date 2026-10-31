package live.renoa.app.agents

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.emitAll
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import live.renoa.rcp.PendingTextCommand
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
 * [ChatMessage.notSent] until [retry] or [discard]. A message the outbox held when the chat opened
 * shows until it arrives.
 *
 * Making an agent, reading its profile, the routines and which agent is main are Host control,
 * carried by the same connection ([HostAgents], [HostRoutines], [HostOwner]). The routines and the
 * main agent are read again on every roster check, and each Host agent's purpose until the Host has
 * answered for it.
 * A new agent is an `agent:<id>` target once its node next advertises; [create] waits for that and
 * follows its chat. Abilities and edits are not on the Host's agent API yet, so those calls are
 * refused as [Problem.NotYet] and nothing is sent.
 *
 * Until Renoa first answers, the screens show what [home] kept from the last launch, and each
 * answer is kept there in turn; a call that needs Renoa before then is [Problem.Unavailable].
 */
class RcpAgentRepository(
    private val surface: RcpSurface,
    private val state: RcpState,
    private val scope: CoroutineScope,
    private val retryAfter: (attempt: Int) -> Duration = { attempt -> (1L shl minOf(attempt, 5)).seconds },
    private val rosterEvery: Duration = 1.minutes,
    private val here: () -> ZoneId = { ZoneId.systemDefault() },
    private val clock: TimeSource = TimeSource.Monotonic,
    private val home: SavedHome? = null,
) : AgentRepository {
    private class Room(val target: TargetSummary, val taskId: Uuid) {
        val journal = ChatJournal()
        val conversation = MutableStateFlow(Conversation())

        /** Set once [RcpSurface.attach] returned; from then on [RcpSurface.connect] re-attaches it. */
        var attached = false

        /** What the agent is for, as the Host keeps it; null until read. */
        var purpose: String? = null
    }

    /** The Host's answer to which agent is main: [id] is its Host agent id, or null when the Host has no agent. */
    private class MainAnswer(val id: Uuid?)

    /** Null until the Host has said which agent is main; the last answer stands while it cannot be read. */
    private var main: MainAnswer? = null

    private val lock = Any()
    private val rooms = LinkedHashMap<String, Room>()

    /** The attached agents. The screens see [shown], which keeps the last launch's until the first roster check answers. */
    private val roster = MutableStateFlow<List<AgentSummary>>(emptyList())
    private val shown = ShownHome(home)
    private val unsent =
        UnsentMessages(
            surface,
            state,
            clock,
            shown = { taskId -> synchronized(lock) { rooms.values.any { it.taskId == taskId && it.attached } } },
            dropped = { taskId, change -> synchronized(lock) { rooms.values.firstOrNull { it.taskId == taskId } }?.let { update(it, change) } },
        )
    private val hostClient = HostClient(surface)
    private val host = HostAgents(hostClient)
    private val owner = HostOwner(hostClient)
    private val heldRoutines = HeldRoutines(HostRoutines(hostClient), changed = ::publish)

    /** One [follow] at a time, so the roster check and a new agent never open the same chat twice. */
    private val following = Mutex()

    /** At most one [retryNow] held for the reconnect wait. */
    private val nudges = Channel<Unit>(Channel.CONFLATED)

    private val linkState = MutableStateFlow(Link.Connecting)

    override val link: StateFlow<Link> = linkState.asStateFlow()

    override val agents: StateFlow<List<AgentSummary>> = shown.agents.asStateFlow()

    override val rosterKnown: StateFlow<Boolean> = shown.known

    override val briefing: StateFlow<Briefing?> = MutableStateFlow<Briefing?>(null).asStateFlow()

    override val routines: StateFlow<List<Routine>> = shown.routines.asStateFlow()

    /**
     * Connects, follows every agent's chat, and reconnects after a drop, until [scope] is cancelled
     * or Renoa refuses this phone's sign-in, which no retry can fix.
     */
    fun start() {
        scope.launch { shown.keep() }
        scope.launch {
            var attempt = 0
            var refusals = 0
            while (true) {
                try {
                    surface.connect()
                    unsent.send()
                    follow(surface.listTargets())
                    shown.heard.value = true
                    publish()
                    synchronized(lock) {
                        linkState.value = Link.Connected
                        nudges.tryReceive()
                    }
                    refusals = 0
                    heldRoutines.read()
                    readPurposes()
                    readMainAgent()
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
                                    heldRoutines.read()
                                    readPurposes()
                                    readMainAgent()
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
                unsent.connectionLost()
                // Live text is not replayed, so a draft cut by the drop would be glued to text from after it.
                synchronized(lock) { rooms.values.toList() }.forEach { update(it) { connectionLost() } }
                // A nudge never hurries the retry after a refused sign-in: refused twice running signs the phone out.
                // One that came meanwhile is spent, so it cannot cut the wait after that retry either.
                val wait = retryAfter(attempt++)
                if (refusals > 0) delay(wait).also { nudges.tryReceive() } else withTimeoutOrNull(wait) { nudges.receive() }
            }
        }
    }

    /**
     * For when the owner comes back to the app or the phone changes networks. While connected, it
     * [probes][RcpSurface.probe] the connection, so a dead one is found in seconds. Otherwise it ends
     * the wait before the next attempt, or during one the wait after it, unless the last attempt's
     * sign-in was refused.
     */
    fun retryNow() {
        val connected = synchronized(lock) { (linkState.value == Link.Connected).also { if (!it) nudges.trySend(Unit) } }
        if (connected) scope.launch { runCatching { surface.probe() } }
    }

    /**
     * Opens and attaches the chat of each agent not attached yet; [RcpSurface.connect] re-attaches the
     * rest. A room outlives a failed attach, because the client keeps the cursor of what it applied
     * and replays only what follows. One agent that cannot be opened does not hold up the others; a
     * connection that ends partway, as when a replay stalls, leaves the rest to the next connect.
     */
    private suspend fun follow(targets: List<TargetSummary>) = following.withLock {
        for (target in targets) {
            val room = synchronized(lock) { rooms.getOrPut(agentId(target)) { Room(target, chatTaskId(target.nodeId, target.target)) } }
            if (room.attached) continue
            // Sent before the app last closed: shown until its record arrives. Read once the room exists,
            // so a refusal from here on reaches the room and outlasts this read.
            val held = state.pendingCommands().filter { it.taskId == room.taskId }
            if (held.isNotEmpty()) update(room) { held.forEach { sending(it.commandId, it.text) } }
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
     * Lists the attached agents, marks the one the Host names as main, and names each routine's agent
     * from them. It writes under the lock, so an older roster never overwrites a newer one.
     */
    private fun publish() {
        synchronized(lock) {
            roster.value =
                rooms.filterValues { it.attached }.map { (id, room) ->
                    AgentSummary(id, room.target.name, purpose = room.purpose.orEmpty(), state = room.journal.state, glance = room.journal.glance)
                }
            val named = main
            shown.show(rooms.keys, roster.value) { id -> named?.let { it.id != null && hostAgentId(id) == it.id } }
            val rosterId = { agent: String -> rooms.entries.firstOrNull { (_, room) -> room.attached && room.target.target == "$AGENT_TARGET$agent" }?.key }
            heldRoutines.shown(rosterId, here())?.let { shown.routines.value = it }
        }
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

    /** Reads which agent the Host names as main. While the Host cannot be read, the last answer stands. */
    private suspend fun readMainAgent() {
        val named =
            try {
                owner.mainAgent()
            } catch (failed: RepositoryException) {
                return
            }
        synchronized(lock) { main = MainAnswer(named) }
        publish()
    }

    override suspend fun hasAgents(): Boolean = owner.hasAgents()

    override suspend fun ownerProfile(): OwnerProfile = owner.profile()

    override suspend fun saveOwnerProfile(expectedRevision: String, content: String): OwnerProfile = owner.saveProfile(expectedRevision, content)

    override suspend fun setRoutine(routineId: String, on: Boolean, operationId: String) = heldRoutines.set(routineId, on, operationId)

    /** The agent's chat, once it is followed. One Renoa lists whose chat could not be opened yet is [Problem.Unavailable]. */
    private fun room(agentId: String): Room {
        val room = synchronized(lock) { rooms[agentId]?.takeIf { it.attached } }
        return room ?: throw RepositoryException(notHeld(shown.heard.value && synchronized(lock) { agentId !in rooms }), "no agent $agentId")
    }

    /**
     * Waits for the agent's chat to be followed, as after a restart that reopened it before the first
     * connect. Once Renoa has answered without the agent, it is [Problem.NotFound].
     */
    override fun conversation(agentId: String): Flow<Conversation> =
        flow {
            combine(roster, shown.heard) { agents, heard -> agents.any { it.id == agentId } || (heard && synchronized(lock) { agentId !in rooms }) }.first { it }
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
                if (failure is RcpException && failure.answered) unsent.notTaken(submission.commandId)
                return
            }
            state.removeCommand(submission.commandId)
            update(room) { refused(submission.commandId) }
            throw failure.asProblem()
        }
    }

    /**
     * Shows the message as sending again and sends it now under its own identity. One that cannot go
     * yet waits, as any unsent message does; a refusal takes it out of the chat and throws.
     */
    override suspend fun retry(agentId: String, messageId: String) {
        val room = room(agentId)
        val commandId = Uuid.parseOrNull(messageId) ?: return
        var text: String? = null
        update(room) { text = resend(commandId) }
        val words = text ?: return
        try {
            unsent.retry(PendingTextCommand(commandId, room.taskId, words))
        } catch (refused: RcpException) {
            update(room) { refused(commandId) }
            throw refused.asProblem()
        }
    }

    /** Takes the message out of the outbox and the chat together, even if the caller is cancelled. */
    override suspend fun discard(agentId: String, messageId: String) {
        val room = room(agentId)
        val commandId = Uuid.parseOrNull(messageId) ?: return
        withContext(NonCancellable) {
            var deleted = false
            update(room) { deleted = delete(commandId) }
            if (deleted) {
                state.removeCommand(commandId)
                unsent.forget(commandId)
            }
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

/** Refused sign-ins in a row after which the phone counts as signed out. */
private const val SIGN_OUT_AFTER = 2
