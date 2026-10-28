package live.renoa.app.agents

import live.renoa.app.agents.AbilityKind.Everyday
import live.renoa.app.agents.AbilityKind.Machine
import live.renoa.app.agents.AbilityKind.Soon

/** What a fake agent starts with. */
class AgentSeed(
    val summary: AgentSummary,
    val abilities: List<AbilitySwitch>,
    val messages: List<ChatMessage> = emptyList(),
    val streaming: String? = null,
)

/** The scripted roster the app shows until the real repository exists. */
object FakeWorld {
    private fun ability(id: String, title: String, icon: AbilityIcon, kind: AbilityKind = Everyday, lit: Boolean = false, byAgent: Boolean = false) =
        AbilitySwitch(id, title, icon, kind, lit, byAgent)

    /** The tiles a new agent starts with: three lit, three that only the maker can choose, one that is not built yet. */
    fun starterAbilities(): List<AbilitySwitch> =
        listOf(
            ability("calendar", "Calendar", AbilityIcon.Calendar, lit = true),
            ability("maps", "Maps", AbilityIcon.Maps),
            ability("photos", "Photos", AbilityIcon.Photos),
            ability("music", "Music", AbilityIcon.Music),
            ability("health", "Health", AbilityIcon.Health),
            ability("email", "Email", AbilityIcon.Email),
            ability("search", "Search", AbilityIcon.Search, lit = true),
            ability("browser", "Browser", AbilityIcon.Browser),
            ability("teach", "Teach", AbilityIcon.Teach),
            ability("notes", "Notes", AbilityIcon.Notes, lit = true),
            ability("phone", "Phone", AbilityIcon.Phone, Machine),
            ability("files", "Files", AbilityIcon.Files, Machine),
            ability("commands", "Commands", AbilityIcon.Commands, Machine),
            ability("wallet", "Wallet", AbilityIcon.Wallet, Soon),
            ability("more", "More", AbilityIcon.More),
        )

    private fun lit(vararg ids: String, byAgent: Set<String> = emptySet()): List<AbilitySwitch> =
        starterAbilities().map {
            val on = it.id in ids || it.id in byAgent
            it.copy(lit = on, addedByAgent = it.id in byAgent)
        }

    private fun seed(name: String, state: AgentState, purpose: String, abilities: List<AbilitySwitch> = lit("calendar", "search", "notes")) =
        AgentSeed(AgentSummary(idFor(name), name, purpose, state), abilities)

    fun idFor(name: String): String = name.lowercase().filter { it.isLetterOrDigit() }

    fun demo(): List<AgentSeed> {
        val renoa = seed("Renoa", AgentState.Working, "Runs the day. Knows what the others are up to.")
        return listOf(
            AgentSeed(
                renoa.summary,
                renoa.abilities,
                messages =
                    listOf(
                        ChatMessage("renoa-1", Speaker.Owner, "What's WebMCP, and should we support it? What would it cost to run?"),
                        ChatMessage("renoa-2", Speaker.Agent, "On it. I'll read up, and Kai'Sa can hunt down the costs."),
                    ),
                streaming = "It lets a website offer its own abilities to an agent.",
            ),
            seed("Kai'Sa", AgentState.Working, "Hunts down prices and compares them."),
            seed("Optimus", AgentState.Wants, "Keeps the servers honest."),
            seed("Tifa", AgentState.Resting, "Your training partner. Pushes hard, cheers louder.", lit("calendar", "music", "search", "notes", byAgent = setOf("health"))),
            seed("Nagisa", AgentState.Resting, "Keeps the house running. Groceries, bills, reminders, the plants."),
            seed("Mochi", AgentState.Working, "Sorts the photos and the paperwork."),
            seed("Sensei", AgentState.Resting, "Teaches one new thing a day."),
            seed("Atlas", AgentState.Resting, "Plans trips."),
            seed("Ledger", AgentState.Resting, "Keeps the books."),
            seed("Bard", AgentState.Resting, "Writes things down so you do not have to."),
            seed("Juno", AgentState.Resting, "Watches the calendar."),
            seed("Rook", AgentState.Resting, "Plays chess back."),
            seed("Echo", AgentState.Resting, "Remembers what you said."),
            seed("Saga", AgentState.Resting, "Reads bedtime stories."),
            seed("Nova", AgentState.Asleep, "Looks at the stars."),
            seed("Miso", AgentState.Asleep, "Plans dinner."),
            seed("Pixel", AgentState.Asleep, "Tidies the photo library."),
            seed("Orbit", AgentState.Asleep, "Tracks the packages."),
            seed("Chef", AgentState.Asleep, "Knows the recipes."),
            seed("Vex", AgentState.Asleep, "Argues the other side."),
        )
    }
}
