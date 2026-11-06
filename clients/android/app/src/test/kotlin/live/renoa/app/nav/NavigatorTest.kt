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
    fun aTappedNoticeOpensItsChatOverWhatIsShownOnceAndNeverBeforeHome() {
        val nav = Navigator.starting(Route.Home)
        nav.push(Route.Chat("ledger"))
        nav.openChat("tifa")
        assertEquals(listOf(Route.Home, Route.Chat("ledger"), Route.Chat("tifa")), nav.stack, "back returns to where the owner was")
        nav.openChat("tifa")
        assertEquals(3, nav.stack.size, "a chat already on top is not opened twice")
        nav.openChat("ledger")
        assertEquals(Route.Chat("ledger"), nav.top)

        val signingIn = Navigator.starting(Route.SignIn)
        signingIn.openChat("tifa")
        assertEquals(listOf(Route.SignIn), signingIn.stack, "no chat opens before sign-in")
    }

    @Test
    fun aTappedNoticeClosesAnOpenSheetSoBackNeverBringsItUp() {
        val nav = Navigator.starting(Route.Home)
        nav.push(Route.Edit("tifa"))
        nav.openChat("tifa")
        assertEquals(listOf(Route.Home, Route.Chat("tifa")), nav.stack)
        nav.push(Route.Chat("ledger"))
        nav.push(Route.Edit("ledger"))
        nav.openChat("ledger")
        assertEquals(listOf(Route.Home, Route.Chat("tifa"), Route.Chat("ledger")), nav.stack, "the chat under the sheet is the one asked for")
    }

    @Test
    fun closingASheetTwiceLeavesTheScreenUnderIt() {
        val nav = Navigator.starting(Route.Home)
        nav.push(Route.Chat("tifa"))
        nav.push(Route.Edit("tifa"))
        val sheet = nav.topEntry
        nav.close(sheet)
        nav.close(sheet)
        assertEquals(listOf(Route.Home, Route.Chat("tifa")), nav.stack)
        nav.replace(sheet, Route.Chat("mira"))
        assertEquals(listOf(Route.Home, Route.Chat("tifa")), nav.stack, "a sheet already gone replaces nothing")
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
        nav.replace(nav.topEntry, Route.Chat("mira"))
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
        nav.replace(replaced, Route.Create)
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
