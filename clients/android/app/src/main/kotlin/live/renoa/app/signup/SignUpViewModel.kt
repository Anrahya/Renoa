package live.renoa.app.signup

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import live.renoa.app.agents.AgentRepository
import live.renoa.app.agents.CreateAgent
import live.renoa.app.agents.OwnerProfile
import live.renoa.app.agents.Problem
import live.renoa.app.agents.RepositoryException
import live.renoa.app.agents.wordsOr
import java.util.UUID

/** The main agent sign-up makes. The owner can rename her later. */
internal const val RENOA = "Renoa"
internal const val RENOA_PURPOSE = "Keeps track of things, reminds you, and finds things out."

/** Phrases the owner can tap into their usual day. */
val DAY_PHRASES = listOf("I cook at home", "I work from home", "I'm a student", "I have kids", "I work out")

/**
 * The lines sign-up keeps in the owner's profile, which every agent reads as written. Agents may
 * rewrite the profile later, so the app reads these lines only to resume sign-up and to greet. A
 * change touches one line and leaves every other byte of the profile as it was.
 */
internal object ProfileLines {
    private const val NAME = "Name:"
    private const val DAY = "A usual day:"

    fun name(content: String): String? = valueOf(content, NAME)

    fun day(content: String): String? = valueOf(content, DAY)

    fun withName(content: String, name: String): String = with(content, NAME, name, after = null)

    fun withDay(content: String, day: String): String = with(content, DAY, day, after = NAME)

    private fun valueOf(content: String, key: String): String? =
        content.lineSequence().firstOrNull { it.startsWith(key) }?.removePrefix(key)?.trim()?.ifEmpty { null }

    /**
     * Rewrites the first line for [key] in place. Without one, the line goes under the line for
     * [after], else under a leading front matter block or heading, else at the top.
     */
    private fun with(content: String, key: String, value: String, after: String?): String {
        val crlf = "\r\n" in content
        val line = "$key ${value.lines().joinToString(" ") { it.trim() }.trim()}" + if (crlf) "\r" else ""
        if (content.isEmpty()) return line + "\n"
        val lines = content.split("\n").toMutableList()
        val own = lines.indexOfFirst { it.startsWith(key) }
        if (own >= 0) {
            lines[own] = line
            return lines.joinToString("\n")
        }
        val at =
            after?.let { a -> lines.indexOfFirst { it.startsWith(a) }.takeIf { it >= 0 }?.plus(1) }
                ?: headerEnd(lines)
        lines.add(at, line)
        return lines.joinToString("\n")
    }

    /** Where the profile's own header ends: after front matter between `---` lines, or after a first `#` heading. */
    private fun headerEnd(lines: List<String>): Int {
        if (lines.first().trimEnd() == "---") {
            val close = lines.drop(1).indexOfFirst { it.trimEnd() == "---" }
            if (close >= 0) return close + 2
        }
        return if (lines.first().startsWith("#")) 1 else 0
    }
}

/**
 * The owner's usual day as sentences, so a phrase they tapped in is a whole sentence of its own
 * and can be taken out again without touching their own words.
 */
internal object DayWords {
    private val END = Regex("(?<=[.!?])\\s+")

    private fun sentences(day: String): List<String> = day.trim().split(END).filter { it.isNotBlank() }

    private fun same(sentence: String, phrase: String) = sentence.trim().trimEnd('.', '!', '?').equals(phrase, ignoreCase = true)

    fun has(day: String, phrase: String): Boolean = sentences(day).any { same(it, phrase) }

    fun toggle(day: String, phrase: String): String {
        val all = sentences(day)
        if (all.any { same(it, phrase) }) return all.filterNot { same(it, phrase) }.joinToString(" ")
        val last = all.lastOrNull()
        val closed = if (last == null || last.trimEnd().last() in ".!?") all else all.dropLast(1) + "${last.trimEnd()}."
        return (closed + "$phrase.").joinToString(" ")
    }
}

/** Keeps the id of Renoa's creation across restarts, so a retry after the app was killed still makes one Renoa. */
interface SignUpMemory {
    var operationId: String?
}

enum class SignUpStep { Checking, Name, Day, Meet }

data class SignUpUiState(
    val step: SignUpStep = SignUpStep.Checking,
    val name: String = "",
    val day: String = "",
    val busy: Boolean = false,
    val notice: String? = null,
    /** Renoa is made; the owner can go to Home. */
    val ready: Boolean = false,
    /** The owner already has an agent, as when another phone signed up first: Home, not sign-up. */
    val done: Boolean = false,
) {
    val canName: Boolean get() = !busy && name.isNotBlank()

    /** The phrases the owner's words already hold as sentences of their own. */
    val phrases: Set<String> get() = DAY_PHRASES.filter { DayWords.has(day, it) }.toSet()

    companion object {
        const val MAX_NAME = 60
        const val MAX_DAY = 600
    }
}

/**
 * Sign-up: the owner's name, how a usual day goes, then meeting Renoa. Each answer is saved to the
 * owner's profile before the next screen, so a sign-up stopped partway resumes at the first screen
 * that has nothing saved. Renoa is made under one operation id kept in [memory], so retrying,
 * even after the app was killed, makes her once.
 */
class SignUpViewModel(private val repository: AgentRepository, private val memory: SignUpMemory) : ViewModel() {
    private val _state = MutableStateFlow(SignUpUiState())
    val state: StateFlow<SignUpUiState> = _state.asStateFlow()

    /** The profile as Renoa last returned it. */
    private var profile: OwnerProfile? = null

    init {
        resume()
    }

    fun setName(text: String) = _state.update { it.copy(name = text.take(SignUpUiState.MAX_NAME), notice = null) }

    fun setDay(text: String) = _state.update { it.copy(day = text.take(SignUpUiState.MAX_DAY), notice = null) }

    /** Adds the phrase to the owner's words as a sentence, or takes that sentence out again. */
    fun togglePhrase(phrase: String) = _state.update { it.copy(day = DayWords.toggle(it.day, phrase).take(SignUpUiState.MAX_DAY), notice = null) }

    /** Back from the day to the name. */
    fun back() = _state.update { if (it.step == SignUpStep.Day && !it.busy) it.copy(step = SignUpStep.Name, notice = null) else it }

    /** Reads where sign-up stands, again after a failure to reach Renoa. */
    fun resume() {
        _state.update { it.copy(step = SignUpStep.Checking, busy = true, notice = null) }
        viewModelScope.launch {
            try {
                if (repository.hasAgents()) {
                    finish()
                    _state.update { it.copy(busy = false, done = true) }
                    return@launch
                }
                val read = repository.ownerProfile().also { profile = it }
                val name = ProfileLines.name(read.content)
                when {
                    name == null -> _state.update { it.copy(step = SignUpStep.Name, busy = false) }
                    ProfileLines.day(read.content) == null -> _state.update { it.copy(step = SignUpStep.Day, name = name, busy = false) }
                    else -> {
                        _state.update { it.copy(name = name) }
                        meet()
                    }
                }
            } catch (refused: RepositoryException) {
                _state.update { it.copy(busy = false, notice = refused.wordsOr("Sign-up could not start. Try again.")) }
            }
        }
    }

    fun saveName() {
        val s = _state.value
        if (!s.canName) return
        val name = s.name.trim()
        _state.update { it.copy(busy = true, notice = null) }
        viewModelScope.launch {
            try {
                keep { ProfileLines.withName(it, name) }
                _state.update { it.copy(step = SignUpStep.Day, name = name, busy = false) }
            } catch (refused: RepositoryException) {
                _state.update { it.copy(busy = false, notice = refused.wordsOr("Your name could not be saved. Try again.")) }
            }
        }
    }

    /** Saves the owner's day, or nothing when they skip or leave it empty, and moves on to Renoa. */
    fun saveDay(skip: Boolean = false) {
        val s = _state.value
        if (s.busy) return
        val day = s.day.trim()
        if (skip || day.isEmpty()) {
            meet()
            return
        }
        _state.update { it.copy(busy = true, notice = null) }
        viewModelScope.launch {
            try {
                keep { ProfileLines.withDay(it, day) }
                meet()
            } catch (refused: RepositoryException) {
                _state.update { it.copy(busy = false, notice = refused.wordsOr("That could not be saved. Try again, or skip it.")) }
            }
        }
    }

    /** Makes Renoa, or finds the one an earlier try made. */
    fun meet() {
        _state.update { it.copy(step = SignUpStep.Meet, busy = true, notice = null) }
        viewModelScope.launch {
            try {
                val operationId = memory.operationId ?: UUID.randomUUID().toString().also { remember(it) }
                val lit = repository.starterAbilities().filter { it.lit }.map { it.id }.toSet()
                repository.create(CreateAgent(operationId, RENOA, RENOA_PURPOSE, lit))
                _state.update { it.copy(busy = false, ready = true) }
            } catch (refused: RepositoryException) {
                _state.update { it.copy(busy = false, notice = refused.wordsOr("$RENOA could not be made. Try again.")) }
            }
        }
    }

    /** Keeps [operationId] before the request that carries it leaves; a phone that cannot keep it cannot make Renoa safely. */
    private fun remember(operationId: String) {
        try {
            memory.operationId = operationId
        } catch (full: IllegalStateException) {
            throw RepositoryException(Problem.Unavailable, full.message.orEmpty())
        }
    }

    /** Sign-up is over; a later sign-up, after every agent is gone, makes a new Renoa. */
    fun finish() {
        memory.operationId = null
    }

    /** Applies [change] to the profile; a change made elsewhere meanwhile is read and [change] applied to it once more. */
    private suspend fun keep(change: (String) -> String) {
        val read = profile ?: repository.ownerProfile()
        profile =
            try {
                repository.saveOwnerProfile(read.revision, change(read.content))
            } catch (refused: RepositoryException) {
                if (refused.problem != Problem.Conflict) throw refused
                val again = repository.ownerProfile()
                repository.saveOwnerProfile(again.revision, change(again.content))
            }
    }
}
