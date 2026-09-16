import { createContext, useContext, useMemo, useState, type Dispatch, type ReactNode, type SetStateAction } from "react";
import { initialConfiguration, type Configuration } from "./configuration-model";

type Entry = { saved: Configuration; draft: Configuration };
type Configurations = Record<string, Entry>;
const PreviewConfigurations = createContext<{ entries: Configurations; setEntries: Dispatch<SetStateAction<Configurations>> } | null>(null);

// Owned by the Host preview, so visiting its library or another agent does not
// discard edits. Deliberately in memory: a reload resets the example data.
export function PreviewConfigurationProvider({ children }: { children: ReactNode }) {
  const [entries, setEntries] = useState<Configurations>({});
  return <PreviewConfigurations.Provider value={{ entries, setEntries }}>{children}</PreviewConfigurations.Provider>;
}

export function usePreviewConfiguration(agentId: string, name: string) {
  const context = useContext(PreviewConfigurations);
  if (!context) throw new Error("Agent configuration preview needs its Host provider");
  const initial = useMemo(() => {
    const configuration = initialConfiguration(name);
    return { saved: configuration, draft: configuration };
  }, [name]);
  const entry = context.entries[agentId] ?? initial;
  function update(change: (current: Entry) => Entry) {
    context!.setEntries(entries => ({ ...entries, [agentId]: change(entries[agentId] ?? initial) }));
  }
  return {
    ...entry,
    setDraft: (change: (draft: Configuration) => Configuration) => update(current => ({ ...current, draft: change(current.draft) })),
    save: (value: Configuration) => update(() => ({ saved: value, draft: value })),
    discard: () => update(current => ({ ...current, draft: current.saved })),
  };
}

export type ConfigurationEditor = ReturnType<typeof usePreviewConfiguration>;
