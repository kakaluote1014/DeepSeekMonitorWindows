// 开发预览用的 Tauri IPC 模拟层。
//
// 本项目要跑真实后端需要 Rust + Visual Studio Build Tools，为了让只改界面的
// 贡献者也能直接 `npm run dev` 看到界面，这里在开发模式下用一份演示数据接管
// `window.__TAURI_INTERNALS__`。
//
// 注意：
// - 只有在真正缺少 Tauri IPC 的普通浏览器里才会安装，`npm run tauri:dev`
//   下后端是真实的，不会被覆盖。
// - 生产构建里 import.meta.env.DEV 为 false，整段代码会被静态剔除。

const isTauriRuntime = () => "__TAURI_INTERNALS__" in window;

type UsageModel = {
  key: string;
  name: string;
  totalTokens: number;
  requestCount: number;
  cacheHitTokens: number;
  cacheMissTokens: number;
  responseTokens: number;
  cost: number;
};

type UsageDay = {
  date: string;
  flashTokens: number;
  flashCacheHit: number;
  flashCacheMiss: number;
  flashResponse: number;
  proTokens: number;
  proCacheHit: number;
  proCacheMiss: number;
  proResponse: number;
  visionTokens: number;
  visionCacheHit: number;
  visionCacheMiss: number;
  visionResponse: number;
  totalTokens: number;
  totalCost: number;
};

const dateKey = (date: Date) =>
  `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;

// 演示用的每日曲线：工作日高、周末低，量级接近真实账户。
const dayShape = [0.72, 0.94, 1.18, 0.86, 1.05, 0.38, 0.61];

const buildUsage = () => {
  const today = new Date();
  const days: UsageDay[] = dayShape.map((factor, index) => {
    const date = new Date(today);
    date.setDate(date.getDate() - (dayShape.length - 1 - index));

    const flashHit = Math.round(3_100_000 * factor);
    const flashMiss = Math.round(96_000 * factor);
    const flashResponse = Math.round(21_000 * factor);
    const proHit = Math.round(410_000 * factor);
    const proMiss = Math.round(58_000 * factor);
    const proResponse = Math.round(9_400 * factor);

    const flashTokens = flashHit + flashMiss + flashResponse;
    const proTokens = proHit + proMiss + proResponse;

    return {
      date: dateKey(date),
      flashTokens,
      flashCacheHit: flashHit,
      flashCacheMiss: flashMiss,
      flashResponse,
      proTokens,
      proCacheHit: proHit,
      proCacheMiss: proMiss,
      proResponse,
      visionTokens: 0,
      visionCacheHit: 0,
      visionCacheMiss: 0,
      visionResponse: 0,
      totalTokens: flashTokens + proTokens,
      totalCost: Number((proTokens * 0.0000032 + flashTokens * 0.0000009).toFixed(4)),
    };
  });

  const sum = (pick: (day: UsageDay) => number) => days.reduce((acc, day) => acc + pick(day), 0);

  const models: UsageModel[] = [
    {
      key: "flash",
      name: "V4 Flash",
      totalTokens: sum((day) => day.flashTokens),
      requestCount: 348,
      cacheHitTokens: sum((day) => day.flashCacheHit),
      cacheMissTokens: sum((day) => day.flashCacheMiss),
      responseTokens: sum((day) => day.flashResponse),
      cost: 9.86,
    },
    {
      key: "flashvision",
      name: "V4 Flash Vision",
      totalTokens: 0,
      requestCount: 0,
      cacheHitTokens: 0,
      cacheMissTokens: 0,
      responseTokens: 0,
      cost: 0,
    },
    {
      key: "pro",
      name: "V4 Pro",
      totalTokens: sum((day) => day.proTokens),
      requestCount: 42,
      cacheHitTokens: sum((day) => day.proCacheHit),
      cacheMissTokens: sum((day) => day.proCacheMiss),
      responseTokens: sum((day) => day.proResponse),
      cost: 13.61,
    },
  ];

  return {
    models,
    days,
    monthCost: models.reduce((acc, model) => acc + model.cost, 0),
  };
};

const appConfig = {
  apiKeyConfigured: true,
  apiKeyPreview: "sk-****c0ffee",
  usageTokenConfigured: true,
  refreshIntervalSeconds: 300,
  autoRefreshEnabled: true,
  autostart: true,
  configPath: "%APPDATA%\\DeepSeekMonitorWindows\\config.json",
};

export const installDevMock = () => {
  if (!import.meta.env.DEV || isTauriRuntime()) {
    return;
  }

  const callbacks = new Map<number, (payload: unknown) => void>();
  let nextCallbackId = 1;
  const usage = buildUsage();

  const invoke = async (cmd: string, args?: Record<string, unknown>) => {
    switch (cmd) {
      case "fetch_usage":
        return usage;
      case "fetch_balance":
        return {
          isAvailable: true,
          currency: "CNY",
          totalBalance: "42.80",
          grantedBalance: "0.00",
          toppedUpBalance: "42.80",
        };
      case "get_app_config":
      case "save_api_key":
      case "clear_api_key":
      case "save_usage_token":
      case "clear_usage_token":
      case "save_refresh_interval":
      case "save_auto_refresh_enabled":
      case "save_autostart":
        return appConfig;
      case "start_usage_sync":
        return false;
      case "plugin:app|version":
        return "1.2.0";
      case "plugin:event|listen":
        return nextCallbackId;
      case "plugin:event|unlisten":
        return null;
      default:
        // 窗口、事件等其余调用返回空值即可，界面不依赖它们的返回值。
        void args;
        return null;
    }
  };

  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
    invoke,
    transformCallback: (callback: (payload: unknown) => void, once = false) => {
      const id = nextCallbackId++;
      callbacks.set(id, (payload: unknown) => {
        callback(payload);
        if (once) {
          callbacks.delete(id);
        }
      });
      return id;
    },
    unregisterCallback: (id: number) => {
      callbacks.delete(id);
    },
    convertFileSrc: (path: string) => path,
    metadata: {
      currentWindow: { label: "main" },
      currentWebview: { label: "main" },
    },
  };

  console.info("[dev-mock] 已启用浏览器预览数据，真实 Tauri 后端未连接");
};
