package live.renoa.app.agents

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import live.renoa.rcp.HostMethod
import java.time.Instant
import kotlin.uuid.Uuid

/** What a note on Home is, by its kind in the Host's feed. An ask is no note: it is the agent's [Glance.ask]. */
enum class NoteKind(internal val wire: String) {
    /** An agent tells the owner something. */
    Update("update"),

    /** A routine ran in a conversation of its own. */
    Result("result"),

    /** A routine failed or was skipped; it can run again. */
    Problem("problem"),

    /** An agent turned an ability on for itself. */
    Ability("ability"),
}

/** Something an agent left for the owner on Home. [agentId] is the roster's id, or null while its agent is not on it. */
data class Note(
    val id: String,
    val agentId: String?,
    val kind: NoteKind,
    val title: String,
    val body: String,
    val at: Instant,
) {
    /**
     * What Home shows of it in one line: an update's own words, else its title, which names the
     * routine that finished or failed or the ability turned on; their bodies are the Host's stock words.
     */
    val words: String get() = if (kind == NoteKind.Update) body else title
}

/** Home's line about the day, and the notes left for the owner, newest first. */
data class Feed(val today: String? = null, val notes: List<Note> = emptyList())

@Serializable
internal data class FeedAgent(val id: String)

/** An item of the Host's feed (`GET /v1/host/feed`), with the fields the app reads. */
@Serializable
internal data class FeedItem(
    val id: String,
    val kind: String,
    val agent: FeedAgent,
    val title: String,
    val body: String,
    @SerialName("at_ms") val atMs: Long,
    val done: Boolean,
    /** For an ask or an update, the task it was made in, or the Host session when no task of the node runs it; for a problem, its routine. */
    @SerialName("subject_id") val subjectId: String,
    val choices: List<String>? = null,
)

/** One page of the feed, newest first; [nextBefore] starts the next, older page, or is null on the last. */
@Serializable
internal data class FeedPage(val today: String, val items: List<FeedItem>, @SerialName("next_before") val nextBefore: Long? = null)

/** The owner's feed on the Host: read, dismissed item by item, and a failed routine run again. */
internal class HostFeed(private val host: HostClient) {
    /**
     * Reads every page: the Host keeps a bounded feed, but an open question or problem it never
     * prunes can sit past the first page.
     */
    suspend fun read(): FeedPage {
        val first = page(null)
        val items = first.items.toMutableList()
        var before = first.nextBefore
        while (before != null) {
            val older = page(before)
            items += older.items
            before = older.nextBefore
        }
        return FeedPage(first.today, items)
    }

    private suspend fun page(before: Long?): FeedPage =
        host.call(HostMethod.Get, if (before == null) "/v1/host/feed" else "/v1/host/feed/before/$before", null, FeedPage.serializer())

    suspend fun dismiss(itemId: String) {
        host.call(HostMethod.Post, "/v1/host/feed/items/$itemId/dismissal", null, JsonObject.serializer())
    }

    /** Runs [routineId] now; a repeat of [operationId] runs it once. */
    suspend fun runNow(routineId: String, operationId: String) {
        val body = buildJsonObject { put("operation_id", operationId) }
        host.call(HostMethod.Post, "/v1/host/automations/$routineId/run", body.toString(), JsonObject.serializer())
    }
}

/** An open question from the feed, and the task it was asked in. */
internal class OpenAsk(val itemId: String, val ask: Ask, val taskId: Uuid?)

/**
 * The feed as the Host last listed it, less what the owner has already dealt with here: an
 * answered or dismissed item stays hidden until the Host no longer lists it as open, since a read
 * that started before the change can still show it. [changed] runs after each change.
 */
internal class HeldFeed(private val host: HostFeed, private val changed: () -> Unit) {
    private val lock = Any()
    private var page: FeedPage? = null
    private val dealt = HashSet<String>()

    /** Reads started, and the newest that has landed: a read that answers after a newer one is dropped. */
    private var started = 0L
    private var landed = 0L

    /** Reads the feed again. While the Host cannot be read, the last one stays. */
    suspend fun read() {
        val ticket = synchronized(lock) { ++started }
        val read =
            try {
                host.read()
            } catch (failed: RepositoryException) {
                return
            }
        synchronized(lock) {
            if (ticket < landed) return
            landed = ticket
            page = read
            dealt.retainAll(read.items.filter { !it.done }.map { it.id }.toSet())
        }
        changed()
    }

    /** The open items, newest first, less those dealt with here. */
    private fun open(): List<FeedItem> = synchronized(lock) { page?.items.orEmpty().filter { !it.done && it.id !in dealt } }

    /**
     * Hides [itemId] here at once, and tells the Host. One the Host no longer has is dealt with too;
     * any other failure shows it again and throws.
     */
    suspend fun dismiss(itemId: String) {
        deal(itemId)
        try {
            host.dismiss(itemId)
        } catch (refused: RepositoryException) {
            if (refused.problem == Problem.NotFound) return
            restore(itemId)
            throw refused
        }
    }

    /**
     * Puts away a question answered in another task's place: hidden here from now on, and dismissed
     * on the Host if it can be reached. The answer is already sent, so a failure here is no failure
     * of the answer; the question stays hidden while this launch lasts.
     */
    suspend fun answeredElsewhere(itemId: String) {
        deal(itemId)
        try {
            host.dismiss(itemId)
        } catch (unconfirmed: RepositoryException) {
            // Hidden here either way; the Host keeps it open until a later answer or dismissal.
        }
    }

    /**
     * Runs a problem's routine again and hides the problem here; a run that fails again leaves it
     * open on the Host, so it shows again once a read finds a new failure. A run already waiting
     * counts as run.
     */
    suspend fun runAgain(itemId: String, operationId: String) {
        val item = open().firstOrNull { it.id == itemId && it.kind == NoteKind.Problem.wire } ?: throw RepositoryException(Problem.NotFound, "no problem $itemId")
        try {
            host.runNow(item.subjectId, operationId)
        } catch (refused: RepositoryException) {
            if (refused.problem != Problem.Conflict) throw refused
        }
        deal(itemId)
    }

    /** Hides [itemId] here, as answered or dismissed. */
    fun deal(itemId: String) {
        synchronized(lock) { dealt += itemId }
        changed()
    }

    /** Shows [itemId] again, as when what dealt with it was refused. */
    fun restore(itemId: String) {
        synchronized(lock) { dealt.remove(itemId) }
        changed()
    }

    /** The newest open question of each Host agent, by Host agent id. */
    fun asks(): Map<String, OpenAsk> =
        open()
            .filter { it.kind == "ask" }
            .groupBy { it.agent.id }
            .mapValues { (_, asks) ->
                asks.first().let { OpenAsk(it.id, Ask(it.body, it.choices.orEmpty(), Instant.ofEpochMilli(it.atMs)), Uuid.parseOrNull(it.subjectId)) }
            }

    /** Home's feed, each note's agent named by its roster id; null until the Host has answered. */
    fun shown(rosterId: (hostAgentId: String) -> String?): Feed? {
        val today = synchronized(lock) { page?.today } ?: return null
        val notes =
            open().mapNotNull { item ->
                val kind = NoteKind.entries.firstOrNull { it.wire == item.kind } ?: return@mapNotNull null
                Note(item.id, rosterId(item.agent.id), kind, item.title, item.body, Instant.ofEpochMilli(item.atMs))
            }
        return Feed(today, notes)
    }
}

/**
 * How the owner's answer to a question asked in another task reads in the agent's chat: the chat
 * did not see the question, so the answer names it.
 */
internal fun answerFromElsewhere(question: String, answer: String): String = "About your question “$question”: $answer"
