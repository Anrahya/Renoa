package live.renoa.app.agents

/** The looks the demo agents wear, as a Host might have picked them: no two alike. */
internal val DEMO_LOOKS: Map<String, Look> =
    mapOf(
        "Tifa" to Look(HairFront.Fringe, HairBack.Long, HairColour.Ink, Skin.Light, Eyes.Sparkle, Cheeks.Blush, Wear.None, Tile.Clay, tuft = false),
        "Kai'Sa" to Look(HairFront.Princess, HairBack.None, HairColour.Ink, Skin.Medium, Eyes.Cat, Cheeks.Lines, Wear.Headphones, Tile.Cobalt, tuft = false),
        "Optimus" to Look(HairFront.Short, HairBack.None, HairColour.Grey, Skin.Deep, Eyes.Oval, Cheeks.None, Wear.Glasses, Tile.Cobalt, tuft = true),
        "Nagisa" to Look(HairFront.SidePart, HairBack.TwinTails, HairColour.Auburn, Skin.Fair, Eyes.Lashes, Cheeks.Freckles, Wear.None, Tile.Ochre, tuft = false),
        "Sensei" to Look(HairFront.Short, HairBack.Bun, HairColour.Grey, Skin.Fair, Eyes.Round, Cheeks.None, Wear.Glasses, Tile.Clay, tuft = false),
        "Ledger" to Look(HairFront.Curls, HairBack.None, HairColour.Brown, Skin.Medium, Eyes.Oval, Cheeks.Freckles, Wear.Beanie, Tile.Ochre, tuft = false),
        "Mochi" to Look(HairFront.Fringe, HairBack.None, HairColour.Blonde, Skin.Fair, Eyes.Sparkle, Cheeks.Blush, Wear.CatEars, Tile.Cobalt, tuft = false),
        "Echo" to Look(HairFront.Short, HairBack.None, HairColour.Blonde, Skin.Light, Eyes.Round, Cheeks.Blush, Wear.None, Tile.Ochre, tuft = true),
        "Mira" to Look(HairFront.SidePart, HairBack.None, HairColour.Brown, Skin.Deep, Eyes.Lashes, Cheeks.Blush, Wear.Bow, Tile.Clay, tuft = false),
    )
