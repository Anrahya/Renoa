import { portraitForAgent } from "./host-identity";
import "./styles/host-avatar.css";

export function AgentAvatar({ agentId, name }: { agentId: string; name: string }) {
  const portrait = portraitForAgent(agentId, name);
  return <span className="host-avatar host-avatar-portrait" aria-hidden="true">
    <img src={portrait} alt="" width="96" height="96" decoding="async" loading="lazy" />
  </span>;
}
