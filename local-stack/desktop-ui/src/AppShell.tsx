import React from "react";

/**
 * Fullscreen shell: fixed sidebar + flexible workspace (caller composes main + context inside workspace).
 */
export function AppShell({
  sidebar,
  workspace,
}: {
  sidebar: React.ReactNode;
  workspace: React.ReactNode;
}) {
  return (
    <div className="flex h-screen min-h-0 w-full bg-app text-slate-100 [text-rendering:optimizeLegibility]">
      {sidebar}
      <div className="flex min-h-0 min-w-0 flex-1 flex-col">{workspace}</div>
    </div>
  );
}
