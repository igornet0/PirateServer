import { useCallback, useRef, useState } from "react";
import { CmdVarsModal } from "./CmdVarsModal";
import {
  cmdVarsInvokeArg,
  defaultCmdVarValues,
  fetchCmdPlaceholders,
  type CmdPlaceholder,
} from "./cmdVars";

export function useCmdVarsPrompt(projectPath: string | null, language: string) {
  const [open, setOpen] = useState(false);
  const [placeholders, setPlaceholders] = useState<CmdPlaceholder[]>([]);
  const [values, setValues] = useState<Record<string, string>>({});
  const [title, setTitle] = useState("");
  const resolveRef = useRef<((v: Record<string, string> | null) => void) | null>(null);

  const promptCmdVars = useCallback(
    async (phases: string[], dialogTitle: string): Promise<Record<string, string> | null> => {
      const path = projectPath?.trim();
      if (!path) return {};
      const list = await fetchCmdPlaceholders(path, phases);
      if (list.length === 0) return {};
      return new Promise((resolve) => {
        resolveRef.current = resolve;
        setTitle(dialogTitle);
        setPlaceholders(list);
        setValues(defaultCmdVarValues(list));
        setOpen(true);
      });
    },
    [projectPath],
  );

  const close = (result: Record<string, string> | null) => {
    setOpen(false);
    const r = resolveRef.current;
    resolveRef.current = null;
    r?.(result);
  };

  const modal = (
    <CmdVarsModal
      open={open}
      title={title}
      placeholders={placeholders}
      values={values}
      language={language}
      onChange={(name, value) => setValues((prev) => ({ ...prev, [name]: value }))}
      onConfirm={() => close(cmdVarsInvokeArg(values))}
      onCancel={() => close(null)}
    />
  );

  return { promptCmdVars, cmdVarsModal: modal };
}
