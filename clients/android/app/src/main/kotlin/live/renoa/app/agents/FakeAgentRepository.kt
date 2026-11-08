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
import live.renoa.rcp.Attachment
import java.time.ZonedDateTime
import java.util.UUID
import kotlin.uuid.Uuid

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
    routineSeeds: List<RoutineSeed> = FakeWorld.routines(),
    private val now: () -> ZonedDateTime = { ZonedDateTime.now() },
    feed: Feed = FakeWorld.feed(now().toInstant()),
    private val notes: List<MemoryNote> = FakeWorld.notes(),
    appSeeds: List<AgentApp> = FakeWorld.apps(),
) : AgentRepository {
    private class Room(val conversation: MutableStateFlow<Conversation>, val doing: String) {
        val queue = ArrayDeque<String>()
        var turn: Job? = null

        // Numbering carries on after the seeded messages, so a new message never reuses one's id.
        var nextMessage = conversation.value.messages.size + 1
    }

    private val lock = Any()
    // A seeded question with no time of its own was asked a few minutes ago, so it shows on the day strip.
    private val roster =
        MutableStateFlow(
            seeds.map { seed ->
                val ask = seed.summary.glance.ask
                if (ask == null || ask.at != null) seed.summary else seed.summary.copy(glance = seed.summary.glance.copy(ask = ask.copy(at = now().toInstant().minusSeconds(300))))
            },
        )
    private val abilities = seeds.associate { it.summary.id to it.abilities }.toMutableMap()
    private val rooms =
        seeds.associate { it.summary.id to Room(MutableStateFlow(Conversation(it.messages)), it.doing) }.toMutableMap()
    private val receipts = mutableMapOf<String, Pair<CreateAgent, String>>()

    override val agents: StateFlow<List<AgentSummary>> = roster.asStateFlow()

    /** The fake's roster is known from the start. */
    override val rosterKnown: StateFlow<Boolean> = MutableStateFlow(true).asStateFlow()

    private val scheduled = routineSeeds.associateBy { it.id }
    private val routineOn = routineSeeds.associate { it.id to it.on }.toMutableMap()
    private val routineList = MutableStateFlow(routinesLocked())

    override val routines: StateFlow<List<Routine>> = routineList.asStateFlow()

    private var owner = OwnerProfile("", revisionOf(""))

    private val linkState = MutableStateFlow(Link.Connected)

    override val link: StateFlow<Link> = linkState.asStateFlow()

    /** Sets how the app is connected, as a dropped connection or a sign-out elsewhere would. */
    fun setLink(link: Link) {
        linkState.value = link
    }

    override suspend fun hasAgents(): Boolean = roster.value.isNotEmpty()

    override suspend fun ownerProfile(): OwnerProfile = synchronized(lock) { owner }

    /** As the Host does: the same content again is no change, and a stale revision is a conflict. */
    override suspend fun saveOwnerProfile(expectedRevision: String, content: String): OwnerProfile =
        synchronized(lock) {
            if (content != owner.content) {
                if (expectedRevision != owner.revision) throw RepositoryException(Problem.Conflict, "The profile changed since it was read.")
                owner = OwnerProfile(content, revisionOf(content))
            }
            owner
        }

    /** Setting a routine to what it is already is no change, so a retried change changes it once. */
    override suspend fun setRoutine(routineId: String, on: Boolean, operationId: String) {
        synchronized(lock) {
            if (routineId !in scheduled) throw RepositoryException(Problem.NotFound, "There is no such routine.")
            routineOn[routineId] = on
            routineList.value = routinesLocked()
        }
    }

    override suspend fun memory(): OwnerMemory = OwnerMemory(notes.reversed(), notes.size.toLong())

    private val connected = appSeeds.map { it.copy(on = false) }

    /** Every agent starts with the apps seeded on. */
    private val appsOn = mutableMapOf<String, Set<String>>()
    private val seededOn = appSeeds.filter { it.on }.map { it.connection }.toSet()

    override suspend fun apps(agentId: String): List<AgentApp> = synchronized(lock) { appsLocked(agentId) }

    /** As the Host does: an unknown app is not found, and asking twice is asking once. */
    override suspend fun setApp(agentId: String, connection: String, on: Boolean): List<AgentApp> =
        synchronized(lock) {
            if (connected.none { it.connection == connection }) throw RepositoryException(Problem.NotFound, "There is no such app.")
            val now = appsOn[agentId] ?: seededOn
            appsOn[agentId] = if (on) now + connection else now - connection
            appsLocked(agentId)
        }

    private fun appsLocked(agentId: String): List<AgentApp> {
        if (roster.value.none { it.id == agentId }) throw RepositoryException(Problem.NotFound, "There is no such agent.")
        val on = appsOn[agentId] ?: seededOn
        return connected.map { it.copy(on = it.connection in on) }
    }

    /** Each routine ran at its time today if that has passed, else yesterday, and runs next at the following one. */
    private fun routinesLocked(): List<Routine> {
        val at = now()
        return scheduled.values.map { seed ->
            val today = at.with(seed.at)
            val ran = if (today.isAfter(at)) today.minusDays(1) else today
            val on = routineOn.getValue(seed.id)
            val daily = ObservedSchedule("cron", expression = "${seed.at.minute} ${seed.at.hour} * * *", timezone = at.zone.id)
            Routine(
                seed.id, FakeWorld.idFor(seed.agentName), seed.name, scheduleWords(daily, at.zone), on,
                next = if (on) ran.plusDays(1).toInstant() else null,
                lastRun = if (on) LastRun(ran.toInstant(), if (seed.fails) RunOutcome.Failed else RunOutcome.Done) else null,
            )
        }
    }

    private val feedState = MutableStateFlow(feed)

    override val feed: StateFlow<Feed> = feedState.asStateFlow()

    /** As the Host does: a note it no longer has is [Problem.NotFound]. */
    override suspend fun dismiss(noteId: String) = takeNote(noteId) { true }

    /** Runs it again at once, and it goes; one that is no problem is [Problem.NotFound]. */
    override suspend fun runAgain(noteId: String) = takeNote(noteId) { it.kind == NoteKind.Problem }

    private fun takeNote(noteId: String, fits: (Note) -> Boolean) {
        synchronized(lock) {
            val note = feedState.value.notes.firstOrNull { it.id == noteId && fits(it) } ?: throw RepositoryException(Problem.NotFound, "There is no such note.")
            feedState.update { it.copy(notes = it.notes - note) }
        }
    }

    override fun conversation(agentId: String): Flow<Conversation> = room(agentId).conversation

    override suspend fun send(agentId: String, text: String, files: List<OutgoingFile>) {
        val room = room(agentId)
        if (text.isBlank() && files.isEmpty()) throw RepositoryException(Problem.Invalid, "A message needs some words in it.")
        val attachments = files.map { Attachment(Uuid.random(), it.name, it.mimeType) }
        synchronized(lock) { keepLocked(agentId, room, text.trim(), attachments) }
    }

    /** Every message here is kept the moment it is sent, so none is ever left unsent. */
    override suspend fun retry(agentId: String, messageId: String) = Unit

    /** As for [retry], there is never one to delete. */
    override suspend fun discard(agentId: String, messageId: String) = Unit

    override suspend fun answer(agentId: String, choice: String) {
        val room = room(agentId)
        synchronized(lock) {
            summaryLocked(agentId)
            if (choice.isBlank()) throw RepositoryException(Problem.Invalid, "An answer needs some words in it.")
            glance(agentId) { it.copy(ask = null, unread = false) }
            keepLocked(agentId, room, choice.trim())
        }
    }

    /** Keeps the owner's words and starts a turn if none is running. Call under the lock. */
    private fun keepLocked(agentId: String, room: Room, text: String, attachments: List<Attachment> = emptyList()) {
        val message = ChatMessage("$agentId-${room.nextMessage++}", Speaker.Owner, text, attachments = attachments)
        room.conversation.update { it.copy(messages = it.messages + message) }
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

    /** A link shaped like Renoa's, each one new; the fake holds no files, so it opens nothing. */
    override suspend fun shareLink(shareId: Uuid): String = "/s/$shareId?ticket=${Uuid.random().toHexString()}${Uuid.random().toHexString()}"

    /** Any eight letters and digits sign in a computer named "MacBook"; the fake has no computers waiting. */
    override suspend fun approveComputer(code: String): String {
        if (code.count { it.isLetterOrDigit() } != 8) throw RepositoryException(Problem.NotFound, "no computer shows that code")
        return "MacBook"
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

private fun revisionOf(content: String): String = UUID.nameUUIDFromBytes(content.toByteArray(Charsets.UTF_8)).toString()
