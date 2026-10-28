package live.renoa.app.nav

import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelStore
import androidx.lifecycle.ViewModelStoreOwner

/**
 * One store of view models per screen entry, so what a screen keeps (a form, the id of the agent
 * it is making, "I am done") lives as long as that entry is on the back stack: it survives a
 * rotation, and the next visit to the same route starts fresh. The activity holds this, which is
 * what lets the stores outlive a rotation. Used from the main thread only.
 */
class EntryStores : ViewModel() {
    private val stores = HashMap<Int, ViewModelStore>()

    /** How many entries hold view models right now. */
    internal val held: Int get() = stores.size

    fun ownerFor(entryId: Int): ViewModelStoreOwner {
        val store = stores.getOrPut(entryId) { ViewModelStore() }
        return object : ViewModelStoreOwner {
            override val viewModelStore = store
        }
    }

    /** The entry has left the back stack and its screen has left the display. */
    fun release(entryId: Int) {
        stores.remove(entryId)?.clear()
    }

    override fun onCleared() {
        stores.values.forEach(ViewModelStore::clear)
        stores.clear()
    }
}
