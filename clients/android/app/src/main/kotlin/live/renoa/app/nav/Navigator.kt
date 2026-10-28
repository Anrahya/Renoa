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

/** The back stack. The bottom entry is where back leaves the app. */
class Navigator private constructor(stack: List<Route>) {
    var stack: List<Route> by mutableStateOf(stack)
        private set

    val top: Route get() = stack.last()

    fun push(route: Route) {
        stack = stack + route
    }

    /** Swaps the top screen, so back skips it. */
    fun replaceTop(route: Route) {
        stack = stack.dropLast(1) + route
    }

    /** Starts over with [route] alone, so back cannot return to what came before. */
    fun resetTo(route: Route) {
        stack = listOf(route)
    }

    /** Goes back one screen. Returns false at the bottom, where the system should take over. */
    fun pop(): Boolean {
        if (stack.size <= 1) return false
        stack = stack.dropLast(1)
        return true
    }

    companion object {
        fun starting(at: Route) = Navigator(listOf(at))

        val Saver: Saver<Navigator, List<String>> =
            Saver(save = { nav -> nav.stack.map(Route::encode) }, restore = { saved -> Navigator(saved.map(Route::parse)) })
    }
}

@Composable
fun rememberNavigator(start: Route): Navigator = rememberSaveable(saver = Navigator.Saver) { Navigator.starting(start) }
