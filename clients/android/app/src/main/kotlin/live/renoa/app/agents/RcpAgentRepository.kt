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
import live.renoa.rcp.RcpErrorCode
import live.renoa.rcp.RcpException
import live.renoa.rcp.RcpState
import live.renoa.rcp.RcpSurface
import live.renoa.rcp.TargetSummary
import kotlin.time.Duration
import kotlin.time.Duration.Companion.minutes
import kotlin.time.Duration.Companion.seconds
import kotlin.uuid.Uuid

/**
 * The screens' Renoa over RCP. The agents are the targets the owner's nodes advertise; each has
 * one chat task ([chatTaskId]), followed with its live text while the app runs.
 *
 * [state] is the outbox [surface] writes to. A message the coordinator refused is dropped; any other
 * failure leaves it there, and it is sent again with the same identity after the next connect and
 * on every roster check.
 *
 * Making and editing agents is Host control, which this connection does not carry yet, so those
 * calls are refused as unavailable.
 */
class RcpAgentRepository(
    private val surface: RcpSurface,
    private val state: RcpState,
    private val scope: CoroutineScope,
    private val retryAfter: (attempt: Int) -> Duration = { attempt -> (1L shl minOf(attempt, 5)).seconds },
    private val rosterEvery: Duration = 1.minutes,
) : AgentRepository {
    private class Room(val target: TargetSummary, val taskId: Uuid) {
        val journal = ChatJournal()
        val conversation = MutableStateFlow(Conversation())

        /** Set once [RcpSurface.attach] returned; from then on [RcpSurface.connect] re-attaches it. */
        var attached = false
    }

    private val lock = Any()
    private val rooms = LinkedHashMap<String, Room>()
    private val roster = MutableStateFlow<List<AgentSummary>>(emptyList())

    override val agents: StateFlow<List<AgentSummary>> = roster.asStateFlow()

    override val briefing: StateFlow<Briefing?> = MutableStateFlow<Briefing?>(null).asStateFlow()

    /** Connects, follows every agent's chat, and reconnects after a drop, until [scope] is cancelled. */
    fun start() {
        scope.launch {
            var attempt = 0
            while (true) {
                try {
                    surface.connect()
                    sendUnsent()
                    follow(surface.listTargets())
                    // Only a connection that got this far resets the wait, so one that fails at once backs off.
                    attempt = 0
                    coroutineScope {
                        // An agent made elsewhere joins, and a message the node could not take yet goes, without a reconnect.
                        val roll =
                            launch {
                                while (true) {
                                    delay(rosterEvery)
                                    sendUnsent()
                                    follow(surface.listTargets())
                                }
                            }
                        surface.awaitDisconnect()
                        roll.cancel()
                    }
                } catch (cancelled: CancellationException) {
                    throw cancelled
                } catch (failure: Exception) {
                    // A connection that failed partway is closed before the next attempt.
                    runCatching { surface.disconnect() }
                }
                // Live text is not replayed, so a draft cut by the drop would be glued to text from after it.
                synchronized(lock) { rooms.values.toList() }.forEach { update(it) { connectionLost() } }
                delay(retryAfter(attempt++))
            }
        }
    }

    /**
     * Sends what the outbox still holds. The client sends in order, so a refusal is for the first one
     * left; anything else stops here and leaves the rest for the next try.
     */
    private suspend fun sendUnsent() {
        while (true) {
            try {
                surface.retryPendingCommands()
                return
            } catch (failure: RcpException) {
                if (!failure.refusesTheRequest) return
                val first = state.pendingCommands().firstOrNull() ?: return
                state.removeCommand(first.commandId)
                synchronized(lock) { rooms.values.firstOrNull { it.taskId == first.taskId } }?.let { room -> update(room) { refused(first.commandId) } }
            }
        }
    }

    /**
     * Opens and attaches the chat of each agent not attached yet; [RcpSurface.connect] re-attaches the
     * rest. A room outlives a failed attach, because the client keeps the cursor of what it applied
     * and replays only what follows. One agent that cannot be opened does not hold up the others.
     */
    private suspend fun follow(targets: List<TargetSummary>) {
        for (target in targets) {
            val room = synchronized(lock) { rooms.getOrPut(agentId(target)) { Room(target, chatTaskId(target.nodeId, target.target)) } }
            if (room.attached) continue
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

    /** Lists the attached agents. It writes under the lock, so an older roster never overwrites a newer one. */
    private fun publish() {
        synchronized(lock) {
            roster.value =
                rooms.filterValues { it.attached }.map { (id, room) ->
                    AgentSummary(id, room.target.name, purpose = "", state = room.journal.state, glance = room.journal.glance)
                }
        }
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
                // Nothing reached the outbox: the client refuses a message while it is not connected.
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
            if (failure !is RcpException || !failure.refusesTheRequest) return
            state.removeCommand(submission.commandId)
            update(room) { refused(submission.commandId) }
            throw failure.asProblem()
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

    override suspend fun profile(agentId: String): AgentProfile = notYet()

    override suspend fun starterAbilities(): List<AbilitySwitch> = notYet()

    override suspend fun create(request: CreateAgent): AgentProfile = notYet()

    override suspend fun edit(agentId: String, edit: EditProfile): AgentProfile = notYet()

    override suspend fun setAbility(agentId: String, abilityId: String, lit: Boolean): AbilitySwitch = notYet()

    private fun notYet(): Nothing = throw RepositoryException(Problem.Unavailable, "Host control does not reach the app yet")

    /**
     * Whether the coordinator answered this request with a refusal that a retry cannot change. An
     * error with no request ends the connection, and one like `node_offline` or `internal` may pass.
     */
    private val RcpException.refusesTheRequest: Boolean
        get() =
            requestId != null &&
                when (code) {
                    RcpErrorCode.InvalidMessage, RcpErrorCode.InvalidRole, RcpErrorCode.NotFound, RcpErrorCode.Conflict -> true
                    RcpErrorCode.AuthenticationFailed, RcpErrorCode.NodeOffline, RcpErrorCode.Internal,
                    RcpErrorCode.ReplayRequired, RcpErrorCode.VersionMismatch,
                    -> false
                }

    private fun RcpException.asProblem(): RepositoryException {
        val problem =
            when (code) {
                RcpErrorCode.NotFound -> Problem.NotFound
                RcpErrorCode.Conflict -> Problem.Conflict
                RcpErrorCode.InvalidMessage -> Problem.Invalid
                else -> Problem.Unavailable
            }
        return RepositoryException(problem, message.orEmpty())
    }

    private fun agentId(target: TargetSummary) = "${target.nodeId}/${target.target}"
}
