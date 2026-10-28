package live.renoa.rcp

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertIs
import kotlin.test.assertTrue
import kotlin.uuid.Uuid

/** The wire shapes, checked against the examples in docs/rcp-json-ws-v0.md itself. */
class WireTest {
    private val bindingDoc = File(System.getProperty("renoa.docs"), "rcp-json-ws-v0.md").readText()

    private val clientTypes =
        setOf("authenticate", "authenticate_ticket", "list_tasks", "list_targets", "open_task", "attach", "submit", "delete_task")
    private val surfaceReceivedTypes =
        setOf("authenticated", "task_list", "target_list", "task_opened", "attached", "command_accepted", "task_deleted", "task_event", "error")

    /** Frames the binding defines that a surface never sends or receives. Listing them forces a decision when the document grows. */
    private val notSurfaceFrames =
        setOf(
            "enroll", "enrolled", "advertise_targets", "execute", "acknowledge_execution",
            "execution_acknowledged", "publish_execution_events", "execution_events_accepted",
        )

    private fun documentFrames(): List<JsonObject> =
        Regex("```json\\n(.*?)\\n```", RegexOption.DOT_MATCHES_ALL)
            .findAll(bindingDoc)
            .map { Json.parseToJsonElement(it.groupValues[1]).jsonObject }
            .filter { "type" in it && it.getValue("type").jsonPrimitive.isString }
            .toList()

    private fun typeOf(frame: JsonObject) = frame.getValue("type").jsonPrimitive.content

    @Test
    fun theDocumentedBindingVersionIsTheOneTheClientSpeaks() {
        val documented = Regex("current binding version is `(\\d+)`").find(bindingDoc)!!.groupValues[1].toInt()
        assertEquals(documented, RCP_BINDING_VERSION)
    }

    @Test
    fun everyDocumentedFrameIsEitherSupportedOrDeclaredNotASurfaceFrame() {
        val frames = documentFrames()
        assertTrue(frames.size > 15, "expected the document's worked examples to be found, found ${frames.size}")
        for (frame in frames) {
            val type = typeOf(frame)
            assertTrue(
                type in clientTypes || type in surfaceReceivedTypes || type in notSurfaceFrames,
                "the binding document defines \"$type\", which this client neither supports nor lists as not a surface frame",
            )
        }
    }

    @Test
    fun everyDocumentedClientFrameIsEncodedExactlyAsDocumented() {
        val documented = documentFrames().filter { typeOf(it) in clientTypes }
        assertEquals(clientTypes, documented.map(::typeOf).toSet(), "the document should show every client frame")
        for (frame in documented) {
            val decoded = wireJson.decodeFromJsonElement(ClientMessage.serializer(), frame)
            assertEquals(frame, wireJson.parseToJsonElement(encodeClientMessage(decoded)), "re-encoding ${typeOf(frame)}")
        }
    }

    @Test
    fun everyDocumentedServerFrameDecodesAndKeepsEveryDocumentedField() {
        val documented = documentFrames().filter { typeOf(it) in surfaceReceivedTypes }
        assertEquals(surfaceReceivedTypes, documented.map(::typeOf).toSet(), "the document should show every surface-received frame")
        for (frame in documented) {
            val decoded = decodeServerMessage(frame.toString())
            val reencoded = wireJson.encodeToJsonElement(ServerMessage.serializer(), decoded)
            assertEquals(frame, reencoded, "round trip of ${typeOf(frame)} dropped or changed a field")
        }
    }

    @Test
    fun aMissingCursorIsSentAsNullNotOmitted() {
        val frame = encodeClientMessage(ClientMessage.Attach(7, Uuid.random(), null))
        assertTrue("\"after_sequence\":null" in frame, frame)
    }

    @Test
    fun textInputOmitsContextAndAuthorWhenAbsent() {
        val frame = encodeClientMessage(ClientMessage.Submit(1, Uuid.random(), Uuid.random(), CommandInput.Text("hi")))
        assertTrue("context" !in frame && "author" !in frame, frame)
    }

    @Test
    fun guestAuthorAndContextAreDecoded() {
        val id = Uuid.random()
        val frame =
            """{"type":"task_event","event":{"eventId":"$id","taskId":"$id","sequence":0,"kind":{"type":"command_submitted","command":
            {"commandId":"$id","principalId":"$id","surface":"discord","target":"agent:a","input":{"type":"text","text":"hi","context":"server 1","author":"guest"}}}}}"""
        val event = (decodeServerMessage(frame) as ServerMessage.TaskEventFrame).event
        val input = ((event.kind as TaskEventKind.CommandSubmitted).command.input) as CommandInput.Text
        assertEquals("server 1", input.context)
        assertEquals(Author.Guest, input.author)
    }

    @Test
    fun aFrameOutsideTheSurfaceVocabularyEndsTheSession() {
        for (frame in listOf("""{"type":"execute"}""", """{"type":"nonsense"}""", "not json", """{"version":13}""")) {
            assertFailsWith<ProtocolViolation>(frame) { decodeServerMessage(frame) }
        }
    }

    @Test
    fun anIdentifierThatIsNotLowercaseCanonicalIsRejected() {
        val upper = "0A000000-0000-0000-0000-000000000001"
        val short = "00000000-0000-0000-0000-00000000001"
        for (id in listOf(upper, short, "not-a-uuid")) {
            assertFailsWith<ProtocolViolation>(id) { decodeServerMessage("""{"type":"task_deleted","request_id":1,"task_id":"$id"}""") }
        }
        assertIs<ServerMessage.TaskDeleted>(
            decodeServerMessage("""{"type":"task_deleted","request_id":1,"task_id":"0a000000-0000-0000-0000-000000000001"}"""),
        )
    }

    @Test
    fun anUnknownErrorCodeOrAuthorIsRejected() {
        assertFailsWith<ProtocolViolation> { decodeServerMessage("""{"type":"error","request_id":null,"code":"teapot","message":"m"}""") }
        val id = Uuid.random()
        val principal =
            """{"type":"task_event","event":{"eventId":"$id","taskId":"$id","sequence":0,"kind":{"type":"command_submitted","command":
            {"commandId":"$id","principalId":"$id","surface":"d","target":"t","input":{"type":"text","text":"x","author":"principal"}}}}}"""
        assertFailsWith<ProtocolViolation> { decodeServerMessage(principal) }
    }
}
