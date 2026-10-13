import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type MaterialResult = { active: boolean; supported: boolean; preview?: boolean };
type MaterialState = "off" | "applying" | "active" | "fallback";

export function useNativeMaterial(theme: string) {
  const [enabled, setEnabled] = useState(() => localStorage.getItem("ui-native-glass") === "on");
  const [state, setState] = useState<MaterialState>("off");
  const [message, setMessage] = useState("使用普通背景");
  const queue = useRef(Promise.resolve());

  useEffect(() => {
    let cancelled = false;
    // Stay readable while applying, and ignore stale replies after a theme change.
    document.documentElement.dataset.nativeMaterial = "off";
    const apply = async () => {
      if (cancelled) return;
      setState("applying");
      setMessage("正在调整桌面材质…");
      try {
        const result = await invoke<MaterialResult>("set_native_material", { enabled, dark: theme === "dark" });
        if (cancelled) return;
        if (enabled && result.active === true && result.supported === true) {
          document.documentElement.dataset.nativeMaterial = "active";
          setState("active");
          setMessage("已启用 Acrylic；透明效果由 Windows 系统设置决定");
        } else {
          setState(enabled ? "fallback" : "off");
          setMessage(!enabled ? "使用普通背景" : result.preview
            ? "浏览器仅预览界面，使用普通背景"
            : "当前系统不支持桌面毛玻璃，使用普通背景");
        }
      } catch {
        if (cancelled) return;
        setState("fallback");
        setMessage("未能调整桌面材质，使用普通背景");
      }
    };
    // Serialize native calls so rapid theme changes cannot leave an old tint active.
    queue.current = queue.current.then(apply, apply);
    return () => { cancelled = true; };
  }, [enabled, theme]);

  const changeEnabled = (next: boolean) => {
    localStorage.setItem("ui-native-glass", next ? "on" : "off");
    setEnabled(next);
  };
  return { enabled, state, message, changeEnabled };
}
