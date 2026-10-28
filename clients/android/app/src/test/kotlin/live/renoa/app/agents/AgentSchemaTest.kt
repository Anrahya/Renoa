package live.renoa.app.agents

import kotlinx.serialization.descriptors.SerialDescriptor
import kotlinx.serialization.descriptors.StructureKind
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import java.io.File
import kotlin.test.Test
import kotlin.test.assertNotNull
import kotlin.test.assertTrue

/** The app reads the Host's agent definition; every field it decodes must be one the Host publishes. */
class AgentSchemaTest {
    private val schema =
        Json.parseToJsonElement(File(System.getProperty("renoa.docs"), "schemas/agent-definition.schema.json").readText()).jsonObject

    @Test
    fun everyFieldTheAppReadsIsInTheHostsSchema() {
        val missing = mutableListOf<String>()
        check(AgentRecord.serializer().descriptor, schema, "", missing)
        assertTrue(missing.isEmpty(), "the Host's agent definition has no ${missing.joinToString()}")
    }

    private fun check(descriptor: SerialDescriptor, node: JsonObject, path: String, missing: MutableList<String>) {
        val properties = resolve(node)["properties"]?.jsonObject
        assertNotNull(properties, "$path is not an object in the schema")
        for (index in 0 until descriptor.elementsCount) {
            val name = descriptor.getElementName(index)
            val field = properties[name]?.jsonObject
            if (field == null) {
                missing += "$path$name"
                continue
            }
            val child = descriptor.getElementDescriptor(index)
            if (child.kind == StructureKind.CLASS) check(child, field, "$path$name.", missing)
        }
    }

    /** Follows a `$ref`, directly or as the one object of an `anyOf` with null. */
    private fun resolve(node: JsonObject): JsonObject {
        val ref =
            node["\$ref"]?.jsonPrimitive?.content
                ?: node["anyOf"]?.jsonArray?.firstNotNullOfOrNull { it.jsonObject["\$ref"]?.jsonPrimitive?.content }
                ?: return node
        return schema.getValue("\$defs").jsonObject.getValue(ref.removePrefix("#/\$defs/")).jsonObject
    }
}
