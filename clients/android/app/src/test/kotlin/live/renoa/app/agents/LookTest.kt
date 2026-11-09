package live.renoa.app.agents

import kotlinx.serialization.json.Json
import kotlin.test.Test
import kotlin.test.assertEquals

class LookTest {
    /** Every choice, as the Host names it (`crates/renoa-local/src/host/appearance/look.rs`). */
    private val hostNames =
        mapOf(
            "hair_front" to listOf("short", "fringe", "side_part", "curls", "princess"),
            "hair_back" to listOf("none", "bun", "long", "twin_tails"),
            "hair_colour" to listOf("ink", "brown", "auburn", "blonde", "grey"),
            "skin" to listOf("fair", "light", "medium", "deep"),
            "eyes" to listOf("round", "oval", "lashes", "sparkle", "cat"),
            "cheeks" to listOf("none", "blush", "freckles", "lines"),
            "wear" to listOf("none", "glasses", "bow", "clip", "beanie", "headphones", "cat_ears"),
            "tile" to listOf("clay", "cobalt", "ochre"),
        )

    private fun look(part: String, choice: String): Look {
        val parts = hostNames.mapValues { (name, choices) -> if (name == part) choice else choices.first() }
        val json = parts.entries.joinToString(", ", "{", ", \"tuft\": false}") { (name, value) -> "\"$name\": \"$value\"" }
        return Json.decodeFromString(Look.serializer(), json)
    }

    @Test
    fun everyChoiceTheHostNamesIsOneTheAppDraws() {
        val decoded =
            hostNames.mapValues { (part, choices) ->
                choices.map { choice ->
                    val look = look(part, choice)
                    when (part) {
                        "hair_front" -> look.hairFront
                        "hair_back" -> look.hairBack
                        "hair_colour" -> look.hairColour
                        "skin" -> look.skin
                        "eyes" -> look.eyes
                        "cheeks" -> look.cheeks
                        "wear" -> look.wear
                        else -> look.tile
                    }
                }
            }
        assertEquals(
            mapOf(
                "hair_front" to HairFront.entries,
                "hair_back" to HairBack.entries,
                "hair_colour" to HairColour.entries,
                "skin" to Skin.entries,
                "eyes" to Eyes.entries,
                "cheeks" to Cheeks.entries,
                "wear" to Wear.entries,
                "tile" to Tile.entries,
            ),
            decoded,
            "each Host name decodes to its own choice, and every choice has one",
        )
    }

    @Test
    fun noTwoDemoAgentsLookAlike() {
        assertEquals(DEMO_LOOKS.size, DEMO_LOOKS.values.toSet().size)
    }
}
