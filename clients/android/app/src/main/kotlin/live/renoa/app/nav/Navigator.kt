package live.renoa.app.nav

import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.Saver
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue

/** Every screen the app has. */
sealed interface Route {
    data object SignIn : Route

    data object Home : Route

    data class Chat(val agentId: String) : Route

    data class Edit(val agentId: String) : Route

    data object Create : Route

    /** A string that [parse] turns back into this route, so the stack survives the process being recreated. */
    fun encode(): String =
        when (this) {
            SignIn -> "signin"
            Home -> "home"
            is Chat -> "chat/$agentId"
            is Edit -> "edit/$agentId"
            Create -> "create"
        }

    companion object {
        fun parse(text: String): Route =
            when {
                text == "signin" -> SignIn
                text == "home" -> Home
                text == "create" -> Create
                text.startsWith("chat/") -> Chat(text.removePrefix("chat/"))
                text.startsWith("edit/") -> Edit(text.removePrefix("edit/"))
                else -> throw IllegalArgumentException("not a route: \"$text\"")
            }
    }
}

/** One screen on the back stack. [id] is never reused, so two visits to the same route are two entries. */
data class Entry(val id: Int, val route: Route)

/** The back stack. The bottom entry is where back leaves the app. */
class Navigator private constructor(entries: List<Entry>, private var nextId: Int) {
    var entries: List<Entry> by mutableStateOf(entries)
        private set

    val stack: List<Route> get() = entries.map { it.route }

    val topEntry: Entry get() = entries.last()

    val top: Route get() = topEntry.route

    fun push(route: Route) {
        entries = entries + Entry(nextId++, route)
    }

    /** Swaps the top screen, so back skips it. */
    fun replaceTop(route: Route) {
        entries = entries.dropLast(1) + Entry(nextId++, route)
    }

    /** Starts over with [route] alone, so back cannot return to what came before. */
    fun resetTo(route: Route) {
        entries = listOf(Entry(nextId++, route))
    }

    /** Goes back one screen. Returns false at the bottom, where the system should take over. */
    fun pop(): Boolean {
        if (entries.size <= 1) return false
        entries = entries.dropLast(1)
        return true
    }

    companion object {
        fun starting(at: Route) = Navigator(listOf(Entry(0, at)), 1)

        /** The first item is the next entry id, so a restored stack never reuses the id of a screen that was popped. */
        val Saver: Saver<Navigator, List<String>> =
            Saver(
                save = { nav -> listOf(nav.nextId.toString()) + nav.entries.map { "${it.id} ${it.route.encode()}" } },
                restore = { saved ->
                    val entries =
                        saved.drop(1).map { line ->
                            val (id, route) = line.split(' ', limit = 2)
                            Entry(id.toInt(), Route.parse(route))
                        }
                    Navigator(entries, saved.first().toInt())
                },
            )
    }
}

@Composable
fun rememberNavigator(start: Route): Navigator = rememberSaveable(saver = Navigator.Saver) { Navigator.starting(start) }
