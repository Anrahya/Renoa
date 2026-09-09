import { useCallback, useEffect, useRef, useState } from "react";
import { HostFeed, initialFeed } from "./host-feed";

export function useHost() {
  const [state, setState] = useState(initialFeed);
  const feed = useRef<HostFeed | null>(null);
  useEffect(() => {
    const active = new HostFeed(setState);
    feed.current = active;
    active.start();
    const wake = () => active.refresh();
    const visible = () => { if (document.visibilityState === "visible") wake(); };
    window.addEventListener("online", wake);
    window.addEventListener("focus", wake);
    document.addEventListener("visibilitychange", visible);
    return () => {
      active.stop();
      window.removeEventListener("online", wake);
      window.removeEventListener("focus", wake);
      document.removeEventListener("visibilitychange", visible);
    };
  }, []);
  const refresh = useCallback(() => feed.current?.refresh(), []);
  const lock = useCallback(() => feed.current?.lock(), []);
  return { ...state, refresh, lock };
}
