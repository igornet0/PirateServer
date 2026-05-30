import { useCallback, useState } from "react";
import type { MainTab } from "../SidebarNav";
import { recordTabSwitch } from "../perf/tabSwitchProfiler";

export function useDashboardTabs(initial: MainTab = "projects") {
  const [mainTab, setMainTabState] = useState<MainTab>(initial);

  const setMainTab = useCallback((tab: MainTab) => {
    recordTabSwitch(tab);
    setMainTabState(tab);
  }, []);

  return { mainTab, setMainTab };
}
