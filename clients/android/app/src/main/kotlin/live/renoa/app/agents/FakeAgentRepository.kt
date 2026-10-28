package live.renoa.app.agents

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.job
import kotlinx.coroutines.launch
import java.util.UUID

/** What an agent says back, or that it could not. */
sealed interface Reply {
    data class Words(val text: String) : Reply

    data object Fails : Reply
}

fun interface ReplyScript {
    fun reply(agent: AgentSummary, ownerText: String): Reply

    companion object {
        private val lines =
            listOf(
                "Got it. Let me look into that properly, and I'll come back with something short and useful.",
                "Good question. I'm checking a couple of things first so I don't guess, then I'll tell you what I found.",
                "On it. I'll keep it simple and let you know as soon as there's something worth your time.",
            )

        private val dinner =
            mapOf(
                "Dal and a salad" to "Good pick. Dal and a salad covers the fibre you missed, and it lands around 480 kcal. I'll check in tomorrow morning.",
                "Not sure yet" to "No worries. Something with lentils or greens would balance the day. I'll ask again around nine.",
            )

        /** Ledger always gets stuck, so the stuck badge can be seen. Tifa answers her dinner question. */
        val Demo = ReplyScript { agent, text ->
            when {
                agent.name == "Ledger" -> Reply.Fails
                agent.name == "Tifa" && text in dinner -> Reply.Words(dinner.getValue(text))
                else -> Reply.Words(lines[(agent.name.length + text.length) % lines.size])
            }
        }
    }
}

/**
 * An in-memory stand-in for Renoa that keeps the same rules the real one will: a retried create
 * makes one agent, a refused request leaves nothing behind, stop is final, and machine
 * abilities are chosen only when an agent is made.
 */
class FakeAgentRepository(
    private val scope: CoroutineScope,
    seeds: List<AgentSeed> = FakeWorld.demo(),
    private val wordDelayMs: Long = 60,
    private val script: ReplyScript = ReplyScript.Demo,
    private val newId: () -> String = { UUID.randomUUID().toString() },
    briefing: Briefing? = FakeWorld.briefing(),
) : AgentRepository {
    private class Room(val conversation: MutableStateFlow<Conversation>, val doing: String) {
        val queue = ArrayDeque<String>()
        var turn: Job? = null

        // Numbering carries on after the seeded messages, so a new message never reuses one's id.
        var nextMessage = conversation.value.messages.size + 1
    }

    private val lock = Any()
    private val roster = MutableStateFlow(seeds.map { it.summary })
    private val abilities = seeds.associate { it.summary.id to it.abilities }.toMutableMap()
    private val rooms =
        seeds.associate { it.summary.id to Room(MutableStateFlow(Conversation(it.messages)), it.doing) }.toMutableMap()
    private val receipts = mutableMapOf<String, Pair<CreateAgent, String>>()

    override val agents: StateFlow<List<AgentSummary>> = roster.asStateFlow()

    override val briefing: StateFlow<Briefing?> = MutableStateFlow(briefing).asStateFlow()

    override fun conversation(agentId: String): Flow<Conversation> = room(agentId).conversation

    override suspend fun send(agentId: String, text: String) {
        val room = room(agentId)
        if (text.isBlank()) throw RepositoryException(Problem.Invalid, "A message needs some words in it.")
        synchronized(lock) { keepLocked(agentId, room, text.trim()) }
    }

    override suspend fun answer(agentId: String, choice: String) {
        val room = room(agentId)
        synchronized(lock) {
            val ask = summaryLocked(agentId).glance.ask ?: throw RepositoryException(Problem.Conflict, "There is no question waiting.")
            if (choice !in ask.choices) throw RepositoryException(Problem.Invalid, "\"$choice\" is not one of the answers offered.")
            glance(agentId) { it.copy(ask = null, unread = false) }
            keepLocked(agentId, room, choice)
        }
    }

    /** Keeps the owner's words and starts a turn if none is running. Call under the lock. */
    private fun keepLocked(agentId: String, room: Room, text: String) {
        room.conversation.update { it.copy(messages = it.messages + ChatMessage("$agentId-${room.nextMessage++}", Speaker.Owner, text)) }
        room.queue.addLast(text)
        if (room.turn?.isActive != true) {
            // The turn is recorded before it runs, so even an immediate dispatcher finds itself the room's turn.
            val turn = scope.launch(start = CoroutineStart.LAZY) { takeTurns(agentId, room) }
            room.turn = turn
            turn.start()
        }
    }

    override suspend fun markSeen(agentId: String) {
        synchronized(lock) {
            summaryLocked(agentId)
            glance(agentId) { it.copy(unread = false) }
        }
    }

    override suspend fun stop(agentId: String) {
        val room = room(agentId)
        val turn =
            synchronized(lock) {
                room.queue.clear()
                // A seeded agent can be working with no turn of ours running; stopping it still has to stop it.
                // Everything is settled here, under the lock: a turn that has not seen its cancellation yet finds it is
                // no longer the room's turn and changes nothing, and a message sent right after starts a fresh turn.
                if (summaryLocked(agentId).state == AgentState.Working) endTurnLocked(agentId)
                room.conversation.update { it.copy(streaming = null) }
                room.turn.also { room.turn = null }
            }
        turn?.cancel()
    }

    override suspend fun profile(agentId: String): AgentProfile =
        synchronized(lock) { profileLocked(agentId) }

    override suspend fun starterAbilities(): List<AbilitySwitch> = FakeWorld.starterAbilities()

    override suspend fun create(request: CreateAgent): AgentProfile =
        synchronized(lock) {
            receipts[request.operationId]?.let { (original, id) ->
                if (original != request) throw RepositoryException(Problem.Conflict, "That request was already used for something else.")
                return profileLocked(id)
            }
            checkName(request.name)
            checkPurpose(request.purpose)
            val starter = FakeWorld.starterAbilities()
            val unknown = request.litAbilities - starter.map { it.id }.toSet()
            if (unknown.isNotEmpty()) throw RepositoryException(Problem.Invalid, "Unknown abilities: ${unknown.sorted().joinToString()}.")
            val unavailable = starter.filter { it.id in request.litAbilities && !it.canSwitch(creating = true) }
            if (unavailable.isNotEmpty()) {
                throw RepositoryException(Problem.Invalid, "${unavailable.joinToString { it.title }} cannot be switched on yet.")
            }
            val id = newId()
            roster.update { it + AgentSummary(id, request.name.trim(), request.purpose.trim(), AgentState.Resting) }
            abilities[id] = starter.map { it.copy(lit = it.id in request.litAbilities) }
            rooms[id] = Room(MutableStateFlow(Conversation()), "Thinking about what you said")
            receipts[request.operationId] = request to id
            profileLocked(id)
        }

    override suspend fun edit(agentId: String, edit: EditProfile): AgentProfile =
        synchronized(lock) {
            val current = summaryLocked(agentId)
            if (current.name != edit.expectedName || current.purpose != edit.expectedPurpose) {
                throw RepositoryException(Problem.Conflict, "This agent was changed somewhere else. Open it again to see the latest.")
            }
            checkName(edit.name)
            checkPurpose(edit.purpose)
            roster.update { list ->
                list.map { if (it.id == agentId) it.copy(name = edit.name.trim(), purpose = edit.purpose.trim()) else it }
            }
            profileLocked(agentId)
        }

    override suspend fun setAbility(agentId: String, abilityId: String, lit: Boolean): AbilitySwitch =
        synchronized(lock) {
            summaryLocked(agentId)
            val current = abilities.getValue(agentId)
            val target = current.firstOrNull { it.id == abilityId } ?: throw RepositoryException(Problem.NotFound, "There is no such ability.")
            if (!target.canSwitch(creating = false)) {
                throw RepositoryException(Problem.Invalid, "${target.title} can only be chosen when an agent is made.")
            }
            val updated = target.copy(lit = lit)
            abilities[agentId] = current.map { if (it.id == abilityId) updated else it }
            updated
        }

    private suspend fun takeTurns(agentId: String, room: Room) {
        val me = currentCoroutineContext().job
        // Every step that changes what the owner sees first checks this is still the room's turn: once stop or a
        // failure has ended it, a turn that has not noticed its cancellation yet changes nothing.
        fun mine() = room.turn === me
        synchronized(lock) {
            if (!mine()) return
            setState(agentId, AgentState.Working)
            glance(agentId) { it.copy(doing = room.doing) }
        }
        while (true) {
            // Finishing is decided under the lock, so a message sent right now starts a new turn instead of being stranded.
            val text =
                synchronized(lock) {
                    if (!mine()) return
                    room.queue.removeFirstOrNull().also {
                        if (it == null) {
                            room.turn = null
                            endTurnLocked(agentId)
                        }
                    }
                } ?: return
            val agent = roster.value.first { it.id == agentId }
            delay(wordDelayMs * 6)
            when (val reply = script.reply(agent, text)) {
                is Reply.Words -> if (!speak(agentId, room, reply.text, ::mine)) return
                Reply.Fails -> {
                    synchronized(lock) {
                        if (!mine()) return
                        room.queue.clear()
                        room.turn = null
                        endTurnLocked(agentId, AgentState.Stuck)
                        say(agentId, room, "$agentId-${room.nextMessage++}", "I got stuck on that one. Can you try again later?")
                    }
                    return
                }
            }
        }
    }

    /** Streams [text] word by word, then keeps it. Returns false if the turn was ended on the way. */
    private suspend fun speak(agentId: String, room: Room, text: String, mine: () -> Boolean): Boolean {
        // The reply's id is chosen before the first word, so the streamed text and the kept message are one item.
        val id = synchronized(lock) { if (!mine()) return false else "$agentId-${room.nextMessage++}" }
        val words = text.split(' ')
        for (count in 1..words.size) {
            delay(wordDelayMs)
            synchronized(lock) {
                if (!mine()) return false
                room.conversation.update { it.copy(streaming = StreamingReply(id, words.take(count).joinToString(" "))) }
            }
        }
        synchronized(lock) {
            if (!mine()) return false
            say(agentId, room, id, text)
        }
        return true
    }

    /** Keeps an agent's message and shows it on Home as unread. Call under the lock. */
    private fun say(agentId: String, room: Room, id: String, text: String) {
        room.conversation.update { it.copy(messages = it.messages + ChatMessage(id, Speaker.Agent, text), streaming = null) }
        glance(agentId) { it.copy(line = text, time = "Just now", unread = true) }
    }

    /**
     * A turn is over. The agent waits on the owner if its question is still open, and rests otherwise, unless
     * [state] says how it ended. Whoever it was working with is no longer working with it. Call under the lock.
     */
    private fun endTurnLocked(agentId: String, state: AgentState? = null) {
        val waiting = summaryLocked(agentId).glance.ask != null
        setState(agentId, state ?: if (waiting) AgentState.Wants else AgentState.Resting)
        roster.update { list ->
            list.map {
                when {
                    it.id == agentId -> it.copy(glance = it.glance.copy(doing = null, with = null))
                    it.glance.with == agentId -> it.copy(glance = it.glance.copy(with = null))
                    else -> it
                }
            }
        }
    }

    private fun glance(agentId: String, change: (Glance) -> Glance) {
        roster.update { list -> list.map { if (it.id == agentId) it.copy(glance = change(it.glance)) else it } }
    }

    private fun setState(agentId: String, state: AgentState) {
        roster.update { list -> list.map { if (it.id == agentId) it.copy(state = state) else it } }
    }

    private fun room(agentId: String): Room =
        synchronized(lock) { rooms[agentId] } ?: throw RepositoryException(Problem.NotFound, "There is no such agent.")

    private fun summaryLocked(agentId: String): AgentSummary =
        roster.value.firstOrNull { it.id == agentId } ?: throw RepositoryException(Problem.NotFound, "There is no such agent.")

    private fun profileLocked(agentId: String): AgentProfile {
        val summary = summaryLocked(agentId)
        return AgentProfile(summary.id, summary.name, summary.purpose, abilities.getValue(agentId))
    }

    private fun checkName(name: String) {
        when (AgentRules.checkName(name)) {
            FieldProblem.Blank -> throw RepositoryException(Problem.Invalid, "A name needs at least one letter.")
            FieldProblem.TooLong -> throw RepositoryException(Problem.Invalid, "A name can be at most ${AgentRules.MAX_NAME_BYTES} bytes long.")
            FieldProblem.ControlCharacter, null -> Unit
        }
    }

    private fun checkPurpose(purpose: String) {
        when (AgentRules.checkPurpose(purpose)) {
            FieldProblem.TooLong -> throw RepositoryException(Problem.Invalid, "A purpose can be at most ${AgentRules.MAX_PURPOSE_BYTES} bytes long.")
            FieldProblem.ControlCharacter -> throw RepositoryException(Problem.Invalid, "A purpose can only hold text and new lines.")
            FieldProblem.Blank, null -> Unit
        }
    }
}
