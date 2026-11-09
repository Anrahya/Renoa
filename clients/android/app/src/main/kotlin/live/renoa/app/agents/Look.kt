package live.renoa.app.agents

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

/**
 * How an agent looks: one choice of each part. The Host picks it when the agent is made and gives
 * no two of the owner's agents the same one, so the app only draws it. The names are the Host's;
 * the colours are the app's.
 */
@Serializable
data class Look(
    @SerialName("hair_front") val hairFront: HairFront,
    @SerialName("hair_back") val hairBack: HairBack,
    @SerialName("hair_colour") val hairColour: HairColour,
    val skin: Skin,
    val eyes: Eyes,
    val cheeks: Cheeks,
    val wear: Wear,
    val tile: Tile,
    /** A stray tuft standing up from the top of the head. */
    val tuft: Boolean,
)

@Serializable
enum class HairFront {
    @SerialName("short") Short,
    @SerialName("fringe") Fringe,
    @SerialName("side_part") SidePart,
    @SerialName("curls") Curls,
    @SerialName("princess") Princess,
}

@Serializable
enum class HairBack {
    @SerialName("none") None,
    @SerialName("bun") Bun,
    @SerialName("long") Long,
    @SerialName("twin_tails") TwinTails,
}

@Serializable
enum class HairColour {
    @SerialName("ink") Ink,
    @SerialName("brown") Brown,
    @SerialName("auburn") Auburn,
    @SerialName("blonde") Blonde,
    @SerialName("grey") Grey,
}

/** Lightest first. */
@Serializable
enum class Skin {
    @SerialName("fair") Fair,
    @SerialName("light") Light,
    @SerialName("medium") Medium,
    @SerialName("deep") Deep,
}

@Serializable
enum class Eyes {
    @SerialName("round") Round,
    @SerialName("oval") Oval,
    @SerialName("lashes") Lashes,
    @SerialName("sparkle") Sparkle,
    @SerialName("cat") Cat,
}

@Serializable
enum class Cheeks {
    @SerialName("none") None,
    @SerialName("blush") Blush,
    @SerialName("freckles") Freckles,
    @SerialName("lines") Lines,
}

@Serializable
enum class Wear {
    @SerialName("none") None,
    @SerialName("glasses") Glasses,
    @SerialName("bow") Bow,
    @SerialName("clip") Clip,
    @SerialName("beanie") Beanie,
    @SerialName("headphones") Headphones,
    @SerialName("cat_ears") CatEars,
}

/** The tile's colour: the landing page's clay, cobalt and ochre. Green and purple are never one. */
@Serializable
enum class Tile {
    @SerialName("clay") Clay,
    @SerialName("cobalt") Cobalt,
    @SerialName("ochre") Ochre,
}
