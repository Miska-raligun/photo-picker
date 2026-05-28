// Tiny in-house i18n. Two languages, ~80 strings. Context + useM hook,
// localStorage-backed preference.

import { createContext, useContext } from "react";

export type Lang = "en" | "zh";

// Inferred from the English bundle below; both `en` and `zh` must structurally
// match this shape.
const enMessages = {
  en: {
    common: {
      appName: "photo-pick",
      tagline:
        "Local two-stage culling for burst-mode photography. Pipeline + VLM, fully on your machine.",
      runsSection: "Tasks",
      emptyRuns: "No tasks yet. Pick a source above and create one.",
      cancel: "Cancel",
      close: "Close",
      apply: "Apply",
      save: "Save",
      reset: "Reset",
      settings: "Settings",
      langLabel: "EN",
      themeDark: "Switch to dark",
      themeLight: "Switch to light",
    },
    scanForm: {
      title: "New task",
      source: "Source",
      sourceDesc: "Folder of photos to cull, or a hand-picked subset.",
      sourcePlaceholder: "/path/to/shoot  (or /mnt/c/... in WSL)",
      output: "Output directory (optional)",
      outputDesc:
        "Where reports and cache go. Leave empty to use in-place mode (no picked/ folder; rejected files can be deleted from source).",
      outputPlaceholder: "leave empty for in-place mode",
      browse: "Browse",
      createTask: "Create task",
      startTask: "Start task",
      runScan: "Start task",
      paramsHeading: "Parameters",
      configureTaskTitle: "Configure task",
      configureTaskDesc: "Pick how aggressively to cull. Defaults work for most shoots.",

      presetSection: "How aggressively to cull",
      presetCustom: "Custom",
      presetAggressiveTitle: "Strict",
      presetAggressiveHint: "Keep only the single best of each near-duplicate set.",
      presetBalancedTitle: "Balanced",
      presetBalancedHint: "Sensible defaults — a few keepers per burst, auto per scene.",
      presetGentleTitle: "Generous",
      presetGentleHint: "Keep more — looser grouping, more survivors per burst.",
      advancedLabel: "Advanced settings",
      advancedHint: "thresholds, GPU, output mode…",
      inPlaceNotice:
        "No output directory set → in-place mode. Picks stay in source; rejected files can be deleted from source after review.",
      withOutputNotice: "Picks will be copied into the output directory.",

      k1Label: "K1 — keep per burst",
      k1Desc:
        "From each burst (Stage A: nearly-identical photos shot in quick succession), keep the top K1 by technical quality.",

      k2Label: "K2 — keep per composition (optional)",
      k2Desc:
        "From each composition group (Stage B: same framing / subject across different bursts), keep the top K2 by final score. Leave empty for auto mode — each group keeps ≥1 photo, plus any extras whose score is within ~5 % of the best (capped at 5/group). Clear winners stay singletons; near-ties keep both.",
      k2Auto: "auto",

      timeKLabel: "Burst time window (× median)",
      timeKDesc:
        "How much wider than the typical photo gap to treat two photos as part of the same burst. 3× median is a safe default. Larger = more photos lumped together.",

      stageAClipLabel: "Burst similarity threshold",
      stageAClipDesc:
        "Two photos within the time window are merged into the same burst only if their CLIP image similarity exceeds this. 0.95 ≈ \"nearly identical\". Lower → looser grouping; higher → only true duplicates merge.",

      stageBClipLabel: "Composition similarity threshold",
      stageBClipDesc:
        "After Stage A picks, photos are re-grouped by visual composition (same scene / framing). Lower → more shots merged as the same composition. 0.93 is a balanced default.",

      minDtLabel: "Min burst gap (seconds)",
      minDtDesc:
        "Lower bound for the burst-merge time window. Two adjacent photos closer than this still need to pass similarity. Defaults to 0.3s — useful for fast 20+ fps bursts.",

      maxDtLabel: "Max burst gap (seconds)",
      maxDtDesc:
        "Upper bound for the burst-merge time window. Prevents two photos minutes apart from joining the same burst even when CLIP says they look identical. 30s default.",

      hashDistLabel: "pHash fallback distance",
      hashDistDesc:
        "Used only when CLIP is off — max Hamming distance between two photos' perceptual hashes for them to merge. 0 = identical bytes; 6 = lenient default.",

      enableClipLabel: "Run CLIP",
      enableClipDesc:
        "Vision model for visual similarity. Required for Stage B composition grouping AND the accurate Stage A burst check. Turning this off falls back to pHash and disables Stage B.",

      enableFaceLabel: "Run face detection",
      enableFaceDesc:
        "Detect faces so portrait scenes get portrait-specific scoring (eye-open, face sharpness etc.). Adds ~15ms per photo on CPU.",

      inPlaceLabel: "In-place mode",
      inPlaceDesc:
        "Don't copy picks into the output folder. Review picks in the UI, then click Apply to delete rejected files from the source (recoverable via OS trash).",

      adaptiveLabel: "Adaptive thresholds",
      adaptiveDesc:
        "Tighten Stage A/B CLIP thresholds for portrait-heavy shoots (avoid merging different people) and loosen for landscape-only (more aggressive grouping). Bias capped at ±0.025.",

      providerLabel: "Execution provider",
      providerDesc:
        "ONNX runtime backend. CPU works everywhere; GPU options need the matching cargo feature compiled in — otherwise the server silently falls back to CPU.",

      thumbEdgeLabel: "Analysis thumbnail size (px)",
      thumbEdgeDesc:
        "Long edge of the in-memory thumbnail fed to CLIP / face / scoring. Larger = sharper face detection on small subjects, much slower. 1024 is a good balance.",

      linkModeLabel: "Output link mode",
      linkModeDesc:
        "How picks are placed in the output folder. Hardlinks are zero-cost when source and output live on the same filesystem.",
      linkHardlink: "Hardlink (recommended, same filesystem)",
      linkCopy: "Copy (safest, uses disk)",
      linkSymlink: "Symlink (smallest, fragile if source moves)",
    },
    settings: {
      title: "Settings",
      vlmHeading: "VLM (Vision-Language Model)",
      vlmDesc: "Used when you click \"Ask VLM why\" on a composition group.",
      modeEnv: "Use server's environment config",
      modeEnvDesc: "Falls back to OPENAI_/ANTHROPIC_ env vars on the server.",
      modeCustom: "Use custom config (stored in this browser)",
      modeCustomWarning:
        "API key is saved to browser localStorage. Anyone with access to this browser profile can read it. Don't enable this on shared machines.",
      provider: "Provider format",
      providerOpenai: "OpenAI-compatible",
      providerAnthropic: "Anthropic",
      baseUrl: "Endpoint URL",
      apiKey: "API key",
      model: "Model",
      preset: "Preset",
      presetNone: "Custom",
      showKey: "Show",
      hideKey: "Hide",
      clearSaved: "Clear saved",
      requiredFields: "URL / API key / model required for custom mode",
    },
    runCard: {
      scanInProgress: "scan in progress",
      scanComplete: "scan complete",
      scanFailed: "scan failed",
      running: "running",
      completed: "completed",
      failed: "failed",
      inPlace: "in-place",
      statPhotos: "photos",
      statCache: "cache",
      statBursts: "bursts",
      statCompGroups: "comp groups",
      statKept: "kept",
      statRejected: "rejected",
      statElapsed: "elapsed",
      openHtmlReport: "Open full HTML report",
      viewResults: "View results",
      taskDetails: "Task details",
      emptyGroups: "No composition groups for this run.",
      starting: "starting…",
    },
    groupCard: {
      keptSuffix: "kept",
    },
    detail: {
      photos: "photos",
      kept: "kept",
      askVlm: "Ask VLM why",
      thinking: "thinking…",
      failed: "failed",
      forceKeep: "＋ Mark to keep",
      forceDrop: "− Mark to delete",
      restoreKeep: "↩ Restore (keep)",
      restoreDrop: "↩ Restore (reject)",
      scoreFinal: "final",
      scoreTech: "tech",
      scoreAesthetic: "aesthetic",
      scoreComposition: "composition",
      scoreFaceBonus: "face bonus",
      verdictWillKeep: "keep",
      verdictWillDrop: "delete",
      verdictForceKeep: "force keep",
      verdictForceDrop: "force delete",
      aiRank: "AI",
      viewOriginal: "View original",
      openInNewTab: "Open in new tab",
      previewFailed: "failed to load preview",
      toggleToReject: "Currently keeping (click to mark for delete)",
      toggleToKeep: "Will delete (click to mark to keep)",
      lightboxHint: "scroll · double-click to zoom · drag to pan · ESC to close",
      prevGroup: "Previous group (←)",
      nextGroup: "Next group (→)",
      groupUnavailable: "This group couldn't be loaded.",
    },
    applyBar: {
      willDelete: "Will delete",
      rejectedFile: "rejected file",
      rejectedFiles: "rejected files",
      fromSource: "from the source.",
      keptByOverride: "kept by override",
      applied: "Applied",
      applyN: (n: number) => `Apply (${n})`,
      confirmTitle: "Apply selection",
      confirmDescPrefix: "About to remove",
      confirmDescSuffix: "files from",
      confirmOverrideNote: "rejected photos will be",
      confirmKeptWord: "kept",
      confirmDueOverride:
        "due to your override. Algorithm picks are untouched.",
      sendToTrash: "Send to system trash (recoverable)",
      deletePermanent: "Delete permanently (no recovery)",
      applyToFiles: (n: number) => `Apply to ${n} files`,
      toastMovedToTrash: "moved to trash",
      toastDeleted: "deleted",
      toastFailedSuffix: "failed",
      toastApplyFailed: "Apply failed",
    },
    browse: {
      sourceTitle: "Select source folder or photos",
      outputTitle: "Select output folder",
      sourceDescription:
        "Browse the server's filesystem. Pick a folder, or check individual photos to scan a subset.",
      outputDescription: "Browse the server's filesystem.",
      foldersHeading: "Folders",
      photosHeading: "Photos",
      noSubfolders: "(no subfolders)",
      noPhotos: "(no photo files here)",
      toggleAll: "Toggle all",
      loading: "loading…",
      useThisFolder: "Use this folder",
      useFolderN: (n: number) => `Use this folder (${n} photos)`,
      useSelectionN: (n: number) => `Use selection (${n} photos)`,
      selectionOfN: (sel: number, total: number) =>
        `${sel} of ${total} selected`,
      photosInFolder: (n: number) => `${n} photos in this folder`,
    },
    errors: {
      pickSource: "Pick a source folder or some files",
      outputRequired: "Output directory required",
      failedToStart: "Failed to start scan",
    },
  },

};

export type Messages = (typeof enMessages)["en"];

export const messages: Record<Lang, Messages> = {
  en: enMessages.en,
  zh: {
    common: {
      appName: "photo-pick 智能选片",
      tagline: "本地化的微单连拍选片助手。两阶段算法 + 视觉大模型解释，全程在你电脑上跑。",
      runsSection: "任务列表",
      emptyRuns: "还没有任务。在上面选好源目录，点击\"创建任务\"。",
      cancel: "取消",
      close: "关闭",
      apply: "执行",
      save: "保存",
      reset: "重置",
      settings: "设置",
      langLabel: "中",
      themeDark: "切换到深色",
      themeLight: "切换到浅色",
    },
    scanForm: {
      title: "新建任务",
      source: "源目录",
      sourceDesc: "要筛选的照片所在文件夹，或手动挑选其中若干张。",
      sourcePlaceholder: "/path/to/shoot（WSL 用 /mnt/c/... 格式）",
      output: "输出目录（可选）",
      outputDesc: "存放报告和缓存的目录。**留空则自动启用原地模式**：不在外部生成 picked/ 文件夹，可在结果页直接删除源目录里被拒的照片。",
      outputPlaceholder: "留空即原地模式",
      browse: "浏览",
      createTask: "创建任务",
      startTask: "开始任务",
      runScan: "开始任务",
      paramsHeading: "算法参数",
      configureTaskTitle: "配置任务",
      configureTaskDesc: "选择筛选的力度即可，默认值适用于大多数情况。",

      presetSection: "筛选力度",
      presetCustom: "自定义",
      presetAggressiveTitle: "严格精选",
      presetAggressiveHint: "每组近似照片只保留最好的一张。",
      presetBalancedTitle: "平衡",
      presetBalancedHint: "推荐默认——每组连拍留几张，按场景自动决定。",
      presetGentleTitle: "宽松保留",
      presetGentleHint: "多留一些——分组更松，每组连拍保留更多。",
      advancedLabel: "高级设置",
      advancedHint: "阈值、GPU、输出方式…",
      inPlaceNotice: "未填输出目录 → 启用原地模式。算法选中的留在源目录，被拒的可在结果页确认后删除。",
      withOutputNotice: "选中的照片会复制到输出目录。",

      k1Label: "K1：每组连拍保留张数",
      k1Desc: "Stage A 把短时间内拍的几乎相同的照片归为一组连拍，从每组里按技术质量保留最好的 K1 张。",

      k2Label: "K2：每个构图保留张数（可选）",
      k2Desc: "Stage B 把构图相似的照片（同场景、不同次按快门）再归一组，按综合评分保留最好的 K2 张。留空 = 自动模式：每组至少 1 张，分数接近最高（5% 以内）的额外保留，每组最多 5 张。一张明显好就只留一张；多张接近就都留。",
      k2Auto: "自动",

      timeKLabel: "连拍时间窗口（× 中位间隔）",
      timeKDesc: "判定\"同一组连拍\"的时间跨度，相对于典型拍照间隔的倍数。默认 3 倍较稳。值越大，更多照片被算作一组连拍。",

      stageAClipLabel: "连拍相似度阈值",
      stageAClipDesc: "时间挨着的两张照片，只有视觉相似度（CLIP 余弦）高过这个值才会合并为同一组连拍。0.95 ≈ \"肉眼几乎看不出区别\"。调低 → 分组更宽松；调高 → 只合并真正的重复。",

      stageBClipLabel: "构图相似度阈值",
      stageBClipDesc: "Stage A 选出来的照片再按视觉构图分组。调低 → 更多照片被视为同一构图；0.93 是平衡值。",

      minDtLabel: "连拍时间窗最小值（秒）",
      minDtDesc: "连拍合并时间窗的下限。相邻两张比这更近的仍需通过相似度判定。默认 0.3 秒，高速连拍（20+ fps）可调小。",

      maxDtLabel: "连拍时间窗最大值（秒）",
      maxDtDesc: "连拍合并时间窗的上限。即使 CLIP 判定视觉一致，间隔超过这个值也不会合并。默认 30 秒。",

      hashDistLabel: "pHash 回退距离",
      hashDistDesc: "仅在关闭 CLIP 时使用——两张照片感知哈希的最大汉明距离才合并。0 = 字节相同；6 = 默认宽松。",

      enableClipLabel: "启用 CLIP 视觉模型",
      enableClipDesc: "用于判断照片间的视觉相似度。Stage B 构图分组必需，Stage A 也用它代替粗糙的 pHash。关掉会退回 pHash 且无 Stage B。",

      enableFaceLabel: "启用人脸检测",
      enableFaceDesc: "检测到人脸后会切换到人像评分档（睁眼、人脸清晰度等）。CPU 上每张多耗 ~15ms。",

      inPlaceLabel: "原地操作模式",
      inPlaceDesc: "不在输出目录生成 picked/。在界面里看选片结果，确认后点 Apply 把被拒照片删除（默认进回收站，可恢复）。",

      adaptiveLabel: "自适应阈值",
      adaptiveDesc: "人像多的拍摄自动收紧 Stage A/B 阈值（避免不同人脸被合并），风光多的放宽（更激进合并）。偏移幅度封顶 ±0.025。",

      providerLabel: "推理后端",
      providerDesc: "ONNX 运行时后端。CPU 通用；GPU 选项需要编译时启用对应 cargo feature，否则后台静默退回 CPU。",

      thumbEdgeLabel: "分析缩略图长边（像素）",
      thumbEdgeDesc: "用于 CLIP / 人脸 / 评分的内存缩略图长边。值越大对小人脸越敏感，但速度大幅下降。1024 是平衡值。",

      linkModeLabel: "输出链接方式",
      linkModeDesc: "把入选照片放到输出目录的方式。同一文件系统下硬链接零开销。",
      linkHardlink: "硬链接（推荐，源/输出在同一磁盘）",
      linkCopy: "复制（最稳，占磁盘）",
      linkSymlink: "符号链接（最省，源移动后会失效）",
    },
    settings: {
      title: "设置",
      vlmHeading: "视觉大模型（VLM）",
      vlmDesc: "在结果页点 \"让 AI 解释为什么\" 时调用。",
      modeEnv: "使用服务端环境变量配置",
      modeEnvDesc: "服务进程的 OPENAI_/ANTHROPIC_ 系列环境变量。",
      modeCustom: "在本浏览器配置自定义",
      modeCustomWarning: "API key 会保存在浏览器 localStorage。任何能访问这个浏览器的人都能读取。不要在共用电脑上启用。",
      provider: "Provider 格式",
      providerOpenai: "OpenAI 兼容",
      providerAnthropic: "Anthropic",
      baseUrl: "端点 URL",
      apiKey: "API Key",
      model: "模型名",
      preset: "预设",
      presetNone: "自定义",
      showKey: "显示",
      hideKey: "隐藏",
      clearSaved: "清除已保存",
      requiredFields: "自定义模式需要 URL / API key / 模型名",
    },
    runCard: {
      scanInProgress: "扫描中",
      scanComplete: "扫描完成",
      scanFailed: "扫描失败",
      running: "进行中",
      completed: "完成",
      failed: "失败",
      inPlace: "原地模式",
      statPhotos: "总数",
      statCache: "缓存命中",
      statBursts: "连拍组",
      statCompGroups: "构图组",
      statKept: "保留",
      statRejected: "拒绝",
      statElapsed: "耗时",
      openHtmlReport: "打开完整 HTML 报告",
      viewResults: "查看结果",
      taskDetails: "任务结果",
      emptyGroups: "本任务没有产生构图组。",
      starting: "启动中…",
    },
    groupCard: {
      keptSuffix: "已选",
    },
    detail: {
      photos: "张",
      kept: "已选",
      askVlm: "让 AI 解释为什么",
      thinking: "思考中…",
      failed: "失败",
      forceKeep: "＋ 改为保留",
      forceDrop: "− 改为删除",
      restoreKeep: "↩ 恢复保留",
      restoreDrop: "↩ 恢复拒绝",
      scoreFinal: "综合",
      scoreTech: "技术",
      scoreAesthetic: "美学",
      scoreComposition: "构图",
      scoreFaceBonus: "人脸加成",
      verdictWillKeep: "保留",
      verdictWillDrop: "删除",
      verdictForceKeep: "强制保留",
      verdictForceDrop: "强制删除",
      aiRank: "AI",
      viewOriginal: "查看原图",
      openInNewTab: "在新标签页打开",
      previewFailed: "预览加载失败",
      toggleToReject: "保留中（点击改为删除）",
      toggleToKeep: "将删除（点击改为保留）",
      lightboxHint: "滚轮缩放 · 双击放大 · 拖动平移 · ESC 关闭",
      prevGroup: "上一组（←）",
      nextGroup: "下一组（→）",
      groupUnavailable: "无法加载该分组。",
    },
    applyBar: {
      willDelete: "即将删除",
      rejectedFile: "张被拒照片",
      rejectedFiles: "张被拒照片",
      fromSource: "（来自源目录）。",
      keptByOverride: "张被强制保留",
      applied: "已执行",
      applyN: (n: number) => `执行删除（${n} 张）`,
      confirmTitle: "确认操作",
      confirmDescPrefix: "即将从",
      confirmDescSuffix: "中移除",
      confirmOverrideNote: "张被拒照片由于你的覆盖将被",
      confirmKeptWord: "保留",
      confirmDueOverride: "。算法选中的照片不动。",
      sendToTrash: "送系统回收站（可恢复）",
      deletePermanent: "永久删除（不可恢复）",
      applyToFiles: (n: number) => `确认删除 ${n} 个文件`,
      toastMovedToTrash: "已送回收站",
      toastDeleted: "已删除",
      toastFailedSuffix: "失败",
      toastApplyFailed: "删除失败",
    },
    browse: {
      sourceTitle: "选择源目录或单张照片",
      outputTitle: "选择输出目录",
      sourceDescription: "浏览服务端文件系统。选一个文件夹处理全部，或勾选具体照片处理子集。",
      outputDescription: "浏览服务端文件系统。",
      foldersHeading: "文件夹",
      photosHeading: "照片",
      noSubfolders: "（无子目录）",
      noPhotos: "（这里没有照片文件）",
      toggleAll: "全选/反选",
      loading: "加载中…",
      useThisFolder: "用这个文件夹",
      useFolderN: (n: number) => `用这个文件夹（${n} 张照片）`,
      useSelectionN: (n: number) => `用所选（${n} 张）`,
      selectionOfN: (sel: number, total: number) => `已选 ${sel} / ${total}`,
      photosInFolder: (n: number) => `此目录共 ${n} 张照片`,
    },
    errors: {
      pickSource: "请先选择源目录或照片",
      outputRequired: "需要填写输出目录",
      failedToStart: "扫描启动失败",
    },
  },
};

export const I18nContext = createContext<{
  lang: Lang;
  setLang: (l: Lang) => void;
  m: Messages;
}>({
  lang: "zh",
  setLang: () => {},
  m: messages.zh,
});

export function useM() {
  return useContext(I18nContext).m;
}

export function useI18n() {
  return useContext(I18nContext);
}

export const LANG_STORAGE_KEY = "photo-pick.lang";
