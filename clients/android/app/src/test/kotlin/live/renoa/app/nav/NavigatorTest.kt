package live.renoa.app.nav

import androidx.compose.runtime.saveable.SaverScope
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertNotEquals
import kotlin.test.assertTrue

class NavigatorTest {
    @Test
    fun signingInReplacesTheWholeStackSoBackLeavesTheApp() {
        val nav = Navigator.starting(Route.SignIn)
        nav.resetTo(Route.Home)
        assertEquals(listOf(Route.Home), nav.stack)
        assertFalse(nav.pop(), "back at the bottom belongs to the system")
        assertEquals(Route.Home, nav.top)
    }

    @Test
    fun pushingOpensAScreenAndPoppingReturnsToTheOneBelow() {
        val nav = Navigator.starting(Route.Home)
        nav.push(Route.Chat("tifa"))
        nav.push(Route.Edit("tifa"))
        assertEquals(Route.Edit("tifa"), nav.top)
        assertTrue(nav.pop())
        assertEquals(Route.Chat("tifa"), nav.top)
        assertTrue(nav.pop())
        assertEquals(Route.Home, nav.top)
    }

    @Test
    fun makingAnAgentReplacesTheCreateScreenSoBackSkipsIt() {
        val nav = Navigator.starting(Route.Home)
        nav.push(Route.Create)
        nav.replaceTop(Route.Chat("mira"))
        assertEquals(listOf(Route.Home, Route.Chat("mira")), nav.stack)
        nav.pop()
        assertEquals(Route.Home, nav.top)
    }

    @Test
    fun everyRouteSurvivesBeingSavedAndRestored() {
        val routes = listOf(Route.SignIn, Route.Home, Route.Create, Route.Chat("tifa"), Route.Edit("kaisa"), Route.Chat("odd/id with spaces"))
        for (route in routes) assertEquals(route, Route.parse(route.encode()))
    }

    @Test
    fun aSavedStackComesBackInTheSameOrder() {
        val nav = Navigator.starting(Route.Home)
        nav.push(Route.Chat("tifa"))
        nav.push(Route.Edit("tifa"))
        val restored = Navigator.Saver.restore(saved(nav))!!
        assertEquals(nav.entries, restored.entries)
    }

    private fun saved(nav: Navigator): List<String> = with(Navigator.Saver) { SaverScope { true }.save(nav)!! }

    @Test
    fun everyVisitToAScreenIsAnEntryOfItsOwn() {
        val nav = Navigator.starting(Route.Home)
        nav.push(Route.Create)
        val first = nav.topEntry
        nav.pop()
        nav.push(Route.Create)
        assertEquals(first.route, nav.topEntry.route)
        assertNotEquals(first.id, nav.topEntry.id, "a second visit must not share what the first one kept")
        val replaced = nav.topEntry
        nav.replaceTop(Route.Create)
        assertNotEquals(replaced.id, nav.topEntry.id)
    }

    @Test
    fun aRestoredStackNeverReusesTheIdOfAScreenThatWasPopped() {
        val nav = Navigator.starting(Route.Home)
        nav.push(Route.Create)
        val popped = nav.topEntry
        nav.pop()
        val restored = Navigator.Saver.restore(saved(nav))!!
        restored.push(Route.Create)
        assertNotEquals(popped.id, restored.topEntry.id)
    }

    @Test
    fun aRouteNobodyRecognisesIsRefused() {
        assertFailsWith<IllegalArgumentException> { Route.parse("settings") }
    }
}
