import { useEffect, type ReactNode } from "react";
import { ArrowClockwise, ArrowSquareOut, CirclesThree, Cube, House, Plugs, SignOut, Stack } from "@phosphor-icons/react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import { TooltipProvider } from "@/components/ui/tooltip";
import { Sidebar, SidebarContent, SidebarFooter, SidebarGroup, SidebarGroupLabel, SidebarHeader,
  SidebarInset, SidebarMenu, SidebarMenuBadge, SidebarMenuButton, SidebarMenuItem,
  SidebarProvider, SidebarTrigger, useSidebar } from "@/components/ui/sidebar";
import type { HostRoute } from "./host-navigation";
import type { HostSnapshot } from "./host-contract";
import { agentHref, displayName, isEarlier, timestamp } from "./host-presentation";
import type { useHost } from "./use-host";
type HostStatus = ReturnType<typeof useHost>["status"];
import "./styles/design-system.css";

const destinations = [
  { view: "agents", label: "Agents", icon: CirclesThree },
  { view: "work", label: "Work", icon: Stack },
  { view: "library", label: "Connections", icon: Plugs },
  { view: "overview", label: "System", icon: Cube },
] as const;

export function HostShell({ route, snapshot, status, preview, receivedAt, refresh, logout, children, designPreview = false }: {
  route: HostRoute; snapshot: HostSnapshot; status: HostStatus; preview: boolean; designPreview?: boolean;
  receivedAt: number | null; refresh: () => void; logout: () => void; children: ReactNode;
}) {
  const name = snapshot.agents.find(agent => agent.id === route.agent)?.name;
  const label = destinations.find(item => item.view === route.view)?.label;
  return <div className="renoa-ui dark min-h-svh">
    <a className="sr-only focus:not-sr-only focus:fixed focus:top-3 focus:left-3 focus:z-50 focus:rounded-md focus:bg-background focus:p-3" href="#host-main" onClick={event => {
      event.preventDefault();
      const main = document.getElementById("host-main");
      if (main) { main.tabIndex = -1; main.focus(); }
    }}>Skip to content</a>
    <TooltipProvider><SidebarProvider>
      <HostSidebar {...{ route, snapshot, status, preview, receivedAt, logout }} />
      <SidebarInset className="min-w-0">
        <header className="sticky top-0 z-20 flex h-14 shrink-0 items-center gap-3 border-b bg-background px-4 md:px-6">
          <SidebarTrigger />
          <Separator orientation="vertical" className="h-4 self-center" />
          <nav aria-label="Breadcrumb" className="flex min-w-0 items-center gap-2 text-sm">
            {name ? <><a className="text-muted-foreground hover:text-foreground" href="#agents">Agents</a><span className="text-muted-foreground">/</span><span className="truncate">{displayName(name)}</span></> : <span>{label}</span>}
          </nav>
          <div className="ml-auto flex shrink-0 items-center gap-2">
            <Badge variant="outline">{designPreview ? "Design preview" : preview ? "Read-only preview" : status === "connected" ? "Host connected" : "Reconnecting"}</Badge>
            {!preview && <Button variant="ghost" size="icon" aria-label="Refresh Host" title="Refresh Host" onClick={refresh}><ArrowClockwise /></Button>}
          </div>
        </header>
        {children}
      </SidebarInset>
    </SidebarProvider></TooltipProvider>
  </div>;
}

function HostSidebar({ route, snapshot, status, preview, receivedAt, logout }: {
  route: HostRoute; snapshot: HostSnapshot; status: HostStatus; preview: boolean;
  receivedAt: number | null; logout: () => void;
}) {
  const { setOpenMobile } = useSidebar();
  const agents = snapshot.agents.filter(agent => !isEarlier(agent));
  useEffect(() => { setOpenMobile(false); }, [route.view, route.agent, setOpenMobile]);
  return <Sidebar collapsible="icon">
    <SidebarHeader className="p-3">
      <SidebarMenu><SidebarMenuItem><SidebarMenuButton size="lg" asChild tooltip="Renoa home">
        <a href="/" aria-label="Renoa home"><CirclesThree weight="fill" /><span className="flex flex-col gap-0.5"><strong className="font-semibold">renoa</strong><span className="text-xs text-muted-foreground">Cloud Host</span></span></a>
      </SidebarMenuButton></SidebarMenuItem></SidebarMenu>
    </SidebarHeader>
    <SidebarContent>
      <SidebarGroup>
        <SidebarGroupLabel>Workspace</SidebarGroupLabel>
        <nav aria-label="Host navigation"><SidebarMenu>
          {destinations.map(({ view, label, icon: Icon }) => <SidebarMenuItem key={view}>
            <SidebarMenuButton asChild isActive={route.view === view} tooltip={label}>
              <a href={`#${view}`} aria-current={route.view === view ? "page" : undefined} onClick={() => setOpenMobile(false)}><Icon /><span>{label}</span></a>
            </SidebarMenuButton>
            {view === "agents" && <SidebarMenuBadge>{agents.length}</SidebarMenuBadge>}
          </SidebarMenuItem>)}
        </SidebarMenu></nav>
      </SidebarGroup>
      {agents.length > 0 && <SidebarGroup>
        <SidebarGroupLabel>Agents</SidebarGroupLabel>
        <SidebarMenu>{agents.map(agent => <SidebarMenuItem key={agent.id}>
          <SidebarMenuButton asChild tooltip={displayName(agent.name)} isActive={route.agent === agent.id}>
            <a href={agentHref(agent.id)} onClick={() => setOpenMobile(false)}><CirclesThree /><span>{displayName(agent.name)}</span></a>
          </SidebarMenuButton>
        </SidebarMenuItem>)}</SidebarMenu>
      </SidebarGroup>}
    </SidebarContent>
    <SidebarFooter className="p-3">
      <SidebarMenu>
        <SidebarMenuItem><SidebarMenuButton asChild tooltip="Renoa home"><a href="/"><House /><span>Renoa home</span><ArrowSquareOut className="ml-auto" /></a></SidebarMenuButton></SidebarMenuItem>
        {!preview && <SidebarMenuItem><SidebarMenuButton onClick={logout} tooltip="Sign out"><SignOut /><span>Sign out</span></SidebarMenuButton></SidebarMenuItem>}
      </SidebarMenu>
      <Separator />
      <div className="flex flex-col gap-1 px-2 py-2 text-xs text-muted-foreground group-data-[collapsible=icon]:hidden">
        <span>{preview ? "Saved observation" : status === "connected" ? "Connected to your Host" : "Showing saved records"}</span>
        {receivedAt !== null && <time dateTime={new Date(receivedAt).toISOString()}>{timestamp(receivedAt)}</time>}
        <details><summary>Host identity</summary><code className="mt-2 block break-all">{snapshot.host_id}</code></details>
      </div>
    </SidebarFooter>
  </Sidebar>;
}
