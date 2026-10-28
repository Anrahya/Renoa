package live.renoa.app.agents

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
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
        var nextMessage = conversation.value.messages.size + (if (conversation.value.streaming != null) 1 else 0) + 1
    }

    private val lock = Any()
    private val roster = MutableStateFlow(seeds.map { it.summary })
    private val abilities = seeds.associate { it.summary.id to it.abilities }.toMutableMap()
    private val rooms =
        seeds.associate { it.summary.id to Room(MutableStateFlow(Conversation(it.messages, it.streaming)), it.doing) }.toMutableMap()
    private val receipts = mutableMapOf<String, Pair<CreateAgent, String>>()

    override val agents: StateFlow<List<AgentSummary>> = roster.asStateFlow()

    override val briefing: StateFlow<Briefing?> = MutableStateFlow(briefing).asStateFlow()

    override fun conversation(agentId: String): Flow<Conversation> = room(agentId).conversation

    override suspend fun send(agentId: String, text: String) {
        val room = room(agentId)
        if (text.isBlank()) throw RepositoryException(Problem.Invalid, "A message needs some words in it.")
        synchronized(lock) {
            room.conversation.update { it.copy(messages = it.messages + ChatMessage("$agentId-${room.nextMessage++}", Speaker.Owner, text.trim())) }
            room.queue.addLast(text.trim())
            if (room.turn?.isActive != true) {
                room.turn = scope.launch { takeTurns(agentId, room) }
            }
        }
    }

    override suspend fun answer(agentId: String, choice: String) {
        synchronized(lock) {
            val ask = summaryLocked(agentId).glance.ask ?: throw RepositoryException(Problem.Conflict, "There is no question waiting.")
            if (choice !in ask.choices) throw RepositoryException(Problem.Invalid, "\"$choice\" is not one of the answers offered.")
            glance(agentId) { it.copy(ask = null, unread = false) }
        }
        send(agentId, choice)
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
                // Resting is written here, with the queue, so a message sent right after cannot start a turn that this then overwrites.
                setState(agentId, AgentState.Resting, onlyFrom = AgentState.Working)
                glance(agentId) { it.copy(doing = null, with = null) }
                room.turn.also { room.turn = null }
            }
        turn?.cancel()
        room.conversation.update { it.copy(streaming = null) }
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
        synchronized(lock) {
            setState(agentId, AgentState.Working)
            glance(agentId) { it.copy(doing = room.doing) }
        }
        try {
            while (true) {
                // Finishing is decided under the lock, and so is the state it leaves the agent in, so a message sent
                // right now starts a new turn instead of being stranded, and this turn's Resting cannot land on top of that one's Working.
                val text =
                    synchronized(lock) {
                        room.queue.removeFirstOrNull().also {
                            if (it == null) {
                                room.turn = null
                                setState(agentId, AgentState.Resting)
                                glance(agentId) { g -> g.copy(doing = null, with = null) }
                            }
                        }
                    } ?: break
                val agent = roster.value.first { it.id == agentId }
                delay(wordDelayMs * 6)
                when (val reply = script.reply(agent, text)) {
                    is Reply.Words -> speak(agentId, room, reply.text)
                    Reply.Fails -> {
                        synchronized(lock) {
                            room.queue.clear()
                            room.turn = null
                            setState(agentId, AgentState.Stuck)
                            glance(agentId) { it.copy(doing = null, with = null, line = "I got stuck on that one. Can you try again later?", time = "Just now") }
                        }
                        return
                    }
                }
            }
        } catch (cancelled: CancellationException) {
            room.conversation.update { it.copy(streaming = null) }
            throw cancelled
        }
    }

    private suspend fun speak(agentId: String, room: Room, text: String) {
        // The reply's id is chosen before the first word, so the streamed text and the kept message are one item.
        val id = synchronized(lock) { "$agentId-${room.nextMessage++}" }
        val words = text.split(' ')
        for (count in 1..words.size) {
            delay(wordDelayMs)
            room.conversation.update { it.copy(streaming = StreamingReply(id, words.take(count).joinToString(" "))) }
        }
        synchronized(lock) {
            room.conversation.update { it.copy(messages = it.messages + ChatMessage(id, Speaker.Agent, text), streaming = null) }
            glance(agentId) { it.copy(line = text, time = "Just now", unread = true) }
        }
    }

    private fun glance(agentId: String, change: (Glance) -> Glance) {
        roster.update { list -> list.map { if (it.id == agentId) it.copy(glance = change(it.glance)) else it } }
    }

    private fun setState(agentId: String, state: AgentState, onlyFrom: AgentState? = null) {
        roster.update { list ->
            list.map { if (it.id == agentId && (onlyFrom == null || it.state == onlyFrom)) it.copy(state = state) else it }
        }
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
