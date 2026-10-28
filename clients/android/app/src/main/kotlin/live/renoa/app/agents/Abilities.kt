package live.renoa.app.agents

/** The owner-facing pictures for abilities, as 24 by 24 SVG path data. */
enum class AbilityIcon(val path: String) {
    Calendar("M5 6.5h14v12.5H5zM5 10.5h14M9 4v4.5M15 4v4.5"),
    Maps("M12 21s-6-5.5-6-11a6 6 0 0 1 12 0c0 5.5-6 11-6 11zM12 7.5a2.5 2.5 0 1 0 0 5a2.5 2.5 0 1 0 0-5z"),
    Photos("M4 6h16v12H4zM4 15l5-4 4 3 3-2 4 3M15 8.5a1.5 1.5 0 1 0 0.01 0"),
    Music("M9 18V6l11-2v12M9 18a3 3 0 1 1-6 0a3 3 0 1 1 6 0zM20 16a3 3 0 1 1-6 0a3 3 0 1 1 6 0z"),
    Health("M12 20s-7.5-4.6-7.5-10A4.3 4.3 0 0 1 12 7.4A4.3 4.3 0 0 1 19.5 10c0 5.4-7.5 10-7.5 10z"),
    Email("M4 6h16v12H4zM4 7l8 6 8-6"),
    Search("M10.5 4a6.5 6.5 0 1 0 0 13a6.5 6.5 0 1 0 0-13zM15.5 15.5L20 20"),
    Browser("M12 3a9 9 0 1 0 0 18a9 9 0 1 0 0-18zM3 12h18M12 3c3 3.5 3 14.5 0 18M12 3c-3 3.5-3 14.5 0 18"),
    Teach("M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8z"),
    Notes("M6 4h9l3 3v13H6zM9 10h6M9 14h6M9 18h3"),
    Phone("M5 4h4l2 5-2.5 1.5a11 11 0 0 0 5 5L15 13l5 2v4a2 2 0 0 1-2 2A16 16 0 0 1 3 6a2 2 0 0 1 2-2z"),
    Files("M3 6h6l2 2h10v11H3z"),
    Commands("M4 6l6 6-6 6M12 18h8"),
    Wallet("M4 7h15v12H4zM4 7l11-3v3M15 13h4"),
    More("M12 6v12M6 12h12"),
}

/** Marks drawn on a tile corner. */
object AbilityMarks {
    const val SPARK = "M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8z"
    const val LOCK = "M6 11h12v9H6zM8.5 11V8a3.5 3.5 0 0 1 7 0v3"
}

/** Who may switch an ability, and when. */
enum class AbilityKind {
    /** Switchable whenever the owner likes. */
    Everyday,

    /** Reaches the machine: chosen when the agent is made, by its maker, and never by the agent itself. */
    Machine,

    /** Not built yet; shown faint so the owner can see what is coming. */
    Soon,
    ;

    val switchableAfterCreation: Boolean
        get() =
            when (this) {
                Everyday -> true
                Machine, Soon -> false
            }

    val switchableWhileCreating: Boolean
        get() =
            when (this) {
                Everyday, Machine -> true
                Soon -> false
            }
}

/** One lit or unlit switch on an agent. */
data class AbilitySwitch(
    val id: String,
    val title: String,
    val icon: AbilityIcon,
    val kind: AbilityKind,
    val lit: Boolean,
    /** The agent found and switched this on itself. */
    val addedByAgent: Boolean = false,
) {
    fun canSwitch(creating: Boolean): Boolean = if (creating) kind.switchableWhileCreating else kind.switchableAfterCreation
}
