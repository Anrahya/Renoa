package live.renoa.app.agents

import live.renoa.app.agents.AbilityKind.Everyday
import live.renoa.app.agents.AbilityKind.Machine
import live.renoa.app.agents.AbilityKind.Soon
import java.time.LocalTime

/** What a fake agent starts with. [doing] is what it says it is doing while it works on a reply. */
class AgentSeed(
    val summary: AgentSummary,
    val abilities: List<AbilitySwitch>,
    val messages: List<ChatMessage> = emptyList(),
    val doing: String = "Thinking about what you said",
)

/** A scripted routine that runs every day at [at], from which the fake works out its words and runs. */
class RoutineSeed(val id: String, val agentName: String, val name: String, val at: LocalTime, val on: Boolean = true)

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

    private fun seed(
        name: String,
        state: AgentState,
        purpose: String,
        glance: Glance = Glance(),
        doing: String = "Thinking about what you said",
        messages: List<ChatMessage> = emptyList(),
        abilities: List<AbilitySwitch> = lit("calendar", "search", "notes"),
    ) = AgentSeed(AgentSummary(idFor(name), name, purpose, state, glance, primary = idFor(name) == LEAD), abilities, messages, doing)

    fun idFor(name: String): String = name.lowercase().filter { it.isLetterOrDigit() }

    /** The agent whose note opens Home. */
    const val LEAD = "renoa"

    fun briefing(): Briefing =
        Briefing(LEAD, "Evening, Yash. Tifa wants your dinner plan, and Optimus found three projects worth a look tonight.")

    /** What the demo agents do every day. */
    fun routines(): List<RoutineSeed> =
        listOf(
            RoutineSeed("brief", "Renoa", "Morning brief", LocalTime.of(7, 30)),
            RoutineSeed("lunch", "Tifa", "Lunch photo", LocalTime.of(13, 0), on = false),
            RoutineSeed("github", "Optimus", "GitHub check", LocalTime.of(18, 30)),
            RoutineSeed("prices", "Nagisa", "Prices and groceries", LocalTime.of(19, 15)),
            RoutineSeed("posts", "Kai'Sa", "Posts worth a reply", LocalTime.of(20, 0)),
            RoutineSeed("checkin", "Renoa", "Evening check-in", LocalTime.of(21, 0)),
        )

    /** The owner's own day: food, posting, new projects, groceries and deals. No meetings, no flights. */
    fun demo(): List<AgentSeed> =
        listOf(
            seed(
                "Renoa", AgentState.Working, "Keeps an eye on everything and tells you what matters.",
                Glance(doing = "Picking three posts worth your reply", with = "kaisa"),
                doing = "Reading through tonight's posts",
                messages =
                    listOf(
                        ChatMessage("renoa-1", Speaker.Owner, "Can you and Kai'Sa find me three posts I could reply to tonight?"),
                        ChatMessage("renoa-2", Speaker.Agent, "On it. Kai'Sa is looking now, and I'll pick the ones that sound like you."),
                        ChatMessage("renoa-3", Speaker.Agent, "The first one is a thread about running AI on your own phone. Two more coming."),
                    ),
            ),
            seed(
                "Tifa", AgentState.Wants, "Tracks what you eat and keeps you training.",
                Glance(
                    line = "What's the plan for dinner? You were light on fibre at lunch.",
                    time = "7:58 pm",
                    unread = true,
                    ask = Ask("What's the plan for dinner? You were light on fibre at lunch.", listOf("Dal and a salad", "Not sure yet")),
                ),
                doing = "Looking up calorie counts",
                messages =
                    listOf(
                        ChatMessage("tifa-1", Speaker.Owner, "Lunch: rajma chawal, one bowl, and some curd."),
                        ChatMessage("tifa-2", Speaker.Agent, "Nice one. That's about 620 kcal and 22 g of protein, but light on fibre. Some fruit or a salad later would balance it."),
                    ),
                abilities = lit("calendar", "music", "search", "notes", byAgent = setOf("health")),
            ),
            seed(
                "Kai'Sa", AgentState.Working, "Helps you post on Twitter, and nudges you when you go quiet.",
                Glance(doing = "Finding posts worth replying to", with = LEAD),
                doing = "Finding posts worth replying to",
            ),
            seed(
                "Optimus", AgentState.Resting, "Finds the most interesting new projects on GitHub.",
                Glance(line = "Three projects are trending tonight. One runs a whole assistant inside a browser tab.", time = "7:30 pm", unread = true),
                doing = "Reading through today's trending projects",
            ),
            seed(
                "Nagisa", AgentState.Resting, "Keeps the grocery list and watches for sales.",
                Glance(line = "Atta and toor dal are on sale this week. Your list is updated.", time = "6:40 pm"),
                doing = "Checking this week's prices",
            ),
            seed(
                "Sensei", AgentState.Resting, "Teaches you one new thing a day.",
                Glance(line = "Today's one thing: how a chatbot decides which words in your question matter most.", time = "Yesterday"),
                doing = "Finding a good example",
            ),
            seed(
                "Ledger", AgentState.Stuck, "Keeps track of what you spend.",
                Glance(line = "I couldn't open last month's statement. Can you send it again?", time = "Yesterday"),
                doing = "Adding up last month",
            ),
            seed("Mochi", AgentState.Asleep, "Reads you something calm before bed.", Glance(line = "Wakes at 9:30 pm")),
            seed("Echo", AgentState.Asleep, "Checks in with you every morning.", Glance(line = "Wakes tomorrow at 7:00 am")),
        )
}
