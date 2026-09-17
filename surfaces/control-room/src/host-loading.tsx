// Static loading state for Host surfaces. It renders without hooks or Host
// fetches, so it can also stand in as the Suspense fallback while the lazy
// panel chunk is still arriving.
export function HostLoadingState({ reconnecting = false }: { reconnecting?: boolean }) {
  return <main id="host-main" className="host-content host-loading" aria-busy="true">
    <h1>{reconnecting ? "Waiting for your Host" : "Opening your Host"}</h1>
    <p className="host-intro">{reconnecting ? "We’ll reconnect when it returns. Your login stays in this browser." : "Restoring your browser’s remembered session."}</p></main>;
}
