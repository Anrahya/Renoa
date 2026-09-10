import { GithubLogo } from "@phosphor-icons/react";
import { displayName } from "./host-presentation";
import "./styles/host-avatar.css";

// Personal display artwork only. Names never determine tools, ownership, or role.
const portraits = new Map([
  ["arcee", "/assets/identities/arcee-prime.webp"],
  ["rc", "/assets/identities/arcee-prime.webp"],
  ["soundwave", "/assets/identities/soundwave-prime.webp"],
]);

export function AgentAvatar({ name, github = false }: { name: string; github?: boolean }) {
  const portrait = portraits.get(name.trim().toLocaleLowerCase());
  const initials = displayName(name).split(/\s+/).map(part => part[0]).join("").slice(0, 2);
  return <span className={`host-avatar${portrait ? " host-avatar-portrait" : ""}`} aria-hidden="true">
    {portrait ? <img src={portrait} alt="" width="96" height="96" decoding="async" /> : <span className="host-avatar-initials">{initials}</span>}
    {github && <span className="host-avatar-surface"><GithubLogo weight="fill" size={15} /></span>}
  </span>;
}
