# 通知系统分析：倒计时进度条回退 Toast

> 分析时间：2026-07-06
> 项目：CodexPlusPlus（React + Tauri）
> 目标：在 Electron + Vue3 项目中复现相同效果

---

## 一、核心文件定位

| 文件 | 行号 | 职责 |
|------|------|------|
| `apps/codex-plus-manager/src/App.tsx` | 5005–5031 | `NoticeDialog` 组件定义 |
| `apps/codex-plus-manager/src/App.tsx` | 735 | `notice` 状态声明 |
| `apps/codex-plus-manager/src/App.tsx` | 1786–1788 | `showNotice()` 触发函数 |
| `apps/codex-plus-manager/src/styles.css` | 2703–2808 | 全部通知样式 + 动画 |

---

## 二、实现机制拆解

### 2.1 组件结构（NoticeDialog）

```
┌──────────────────────────────────────────────┐
│ ████████████████████████████████████████████ │  ← .toast-progress (3px 高, 绝对定位顶部)
│ ┌──────┬────────────────────────┬──────────┐ │
│ │ ✅Icon│ 标题标题标题标题标题    │    ×     │ │  ← grid: 38px \| 1fr \| auto
│ │       │ 消息内容消息内容消息内容 │  关闭按钮 │ │
│ └──────┴────────────────────────┴──────────┘ │
└──────────────────────────────────────────────┘
```

**定位**：`position: fixed; top: 16px; left: 50%; transform: translateX(-50%); z-index: 50`

### 2.2 倒计时逻辑

```typescript
// React 实现
function NoticeDialog({ notice, onClose }) {
  useEffect(() => {
    const timer = window.setTimeout(onClose, 4200);  // ← 4.2秒后自动关闭
    return () => window.clearTimeout(timer);          // ← 组件卸载时清除定时器
  }, []);
  // ...
}
```

**关键细节**：

- 调用处使用 `key` prop：`key={\`${notice.title}-${notice.message}-${notice.status ?? ""}\`}`——当内容变化时，React 销毁旧组件创建新组件，倒计时自然重置
- 当前实现 **没有** 鼠标悬停暂停功能（`animation-play-state` 未设置，JS 无 pause 逻辑）

### 2.3 进度条动画（"回退"效果的实现）

```css
.toast-progress {
  position: absolute;
  top: 0;
  left: 0;
  width: 100%;
  height: 3px;
  background: hsl(var(--brand-accent));
  transform-origin: left center;                       /* ← 从左向右收缩 */
  animation: toast-progress 4200ms linear forwards;    /* ← 4.2s 线性完成, 保持结束态 */
}

@keyframes toast-progress {
  from { transform: scaleX(1); }    /* 100% 宽度 → 满格 */
  to   { transform: scaleX(0); }    /* 0% 宽度 → "回退/倒退"视觉效果 */
}
```

**"回退"的本质**：进度条使用 `transform: scaleX()` 从 1 线性收缩到 0，视觉上像是"进度条从左向右倒退"。为什么不用 `width: 100% → 0%`？因为 `transform: scaleX()` 动画在 GPU 合成层执行，性能优于重排属性 `width`。

**失败状态**：`.toast-card.failed .toast-progress` 背景变为红色 `hsl(0 72% 61%)`，图标变为 `Bell` 而非 `CheckCircle2`。

### 2.4 弹入动画

```css
@keyframes toast-in {
  from { opacity: 0; transform: translateY(-10px); }
  to   { opacity: 1; transform: translateY(0); }
}
.toast-card { animation: toast-in 180ms ease-out; }
```

### 2.5 其他样式

| 元素 | 样式要点 |
|------|---------|
| `.toast-card` | `border-radius: 8px; box-shadow: 0 18px 60px rgb(0 0 0 / 0.3)` |
| 布局 | `display: grid; grid-template-columns: 38px minmax(0, 1fr) auto; gap: 12px` |
| `.toast-icon` | `38×38px; border-radius: 8px; background: hsl(var(--accent))` |
| `.toast-body h2` | `font-size: 15px; margin: 0 0 6px` |
| `.toast-body p` | `font-size: 13px; color: hsl(var(--muted-foreground)); overflow-wrap: anywhere` |
| `.toast-close` | `border: 0; background: transparent; font-size: 20px; cursor: pointer` |

---

## 三、Vue3 + Electron 实现方案

### 3.1 整体架构

```
Pinia Store (toasts[])
    │
    ▼
App.vue → <ToastNotification v-for ... />   ← 每个实例独立管理倒计时
                │
                ├── JS: setInterval(100ms) 驱动剩余时间
                ├── CSS: transition 平滑宽度变化
                └── mouseenter/mouseleave 暂停/继续
```

### 3.2 组件代码（ToastNotification.vue）

```vue
<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted } from 'vue'

const props = withDefaults(defineProps<{
  id: number
  title: string
  message: string
  status?: 'success' | 'failed'
  duration?: number
}>(), { duration: 4200, status: 'success' })

const emit = defineEmits<{ (e: 'close', id: number): void }>()

const remaining = ref(props.duration)
const isPaused = ref(false)
let intervalId: number | undefined

const progressWidth = computed(() =>
  `${(remaining.value / props.duration) * 100}%`
)

const progressTransition = computed(() =>
  isPaused.value ? 'none' : 'width 100ms linear'
)

function startTimer() {
  intervalId = window.setInterval(() => {
    if (!isPaused.value) {
      remaining.value = Math.max(0, remaining.value - 100)
      if (remaining.value <= 0) {
        clearInterval(intervalId)
        emit('close', props.id)
      }
    }
  }, 100)
}

function pauseCountdown() { isPaused.value = true }
function resumeCountdown() { isPaused.value = false }

onMounted(startTimer)
onUnmounted(() => clearInterval(intervalId))
</script>

<template>
  <div class="toast-wrap" role="status" aria-live="polite">
    <div
      class="toast-card"
      :class="{ failed: status === 'failed' }"
      @mouseenter="pauseCountdown"
      @mouseleave="resumeCountdown"
    >
      <div
        class="toast-progress"
        :style="{
          width: progressWidth,
          transition: progressTransition,
        }"
      />
      <div class="toast-icon">
        <svg v-if="status === 'failed'" class="h-5 w-5" /* 失败图标 */>…</svg>
        <svg v-else class="h-5 w-5" /* 成功图标 */>…</svg>
      </div>
      <div class="toast-body">
        <h2>{{ title }}</h2>
        <p>{{ message }}</p>
      </div>
      <button class="toast-close" @click="emit('close', id)" type="button">×</button>
    </div>
  </div>
</template>
```

### 3.3 Pinia Store（notificationStore.ts）

```ts
import { defineStore } from 'pinia'

export interface ToastItem {
  id: number
  title: string
  message: string
  status?: 'success' | 'failed'
  duration?: number
}

export const useNotificationStore = defineStore('notification', () => {
  const toasts = ref<ToastItem[]>([])
  let nextId = 0

  function show(title: string, message: string, status?: 'success' | 'failed', duration?: number) {
    toasts.value.push({ id: ++nextId, title, message, status, duration })
  }

  function dismiss(id: number) {
    toasts.value = toasts.value.filter(t => t.id !== id)
  }

  return { toasts, show, dismiss }
})
```

### 3.4 App.vue 中集成

```vue
<template>
  <!-- 其他内容 -->
  <div class="toast-wrap">
    <ToastNotification
      v-for="toast in store.toasts"
      :key="toast.id"
      v-bind="toast"
      @close="store.dismiss"
    />
  </div>
</template>

<script setup lang="ts">
import { useNotificationStore } from '@/stores/notification'
const store = useNotificationStore()

// 使用：store.show('保存成功', '配置已写入', 'success')
</script>
```

### 3.5 CSS 样式（可直接复用）

```scss
.toast-wrap {
  position: fixed;
  top: 16px;
  left: 50%;
  transform: translateX(-50%);
  z-index: 50;
  width: min(420px, calc(100vw - 32px));
}

.toast-card {
  position: relative;
  display: grid;
  grid-template-columns: 38px minmax(0, 1fr) auto;
  gap: 12px;
  align-items: start;
  overflow: hidden;
  border: 1px solid hsl(var(--border));
  border-radius: 8px;
  background: hsl(var(--popover));
  color: hsl(var(--popover-foreground));
  box-shadow: 0 18px 60px hsl(0 0% 0% / 0.3);
  padding: 16px 14px 14px;
  animation: toast-in 180ms ease-out;  /* 弹入动画保留 */
}

.toast-card.failed {
  border-color: hsl(0 72% 51% / 0.45);
}

.toast-progress {
  position: absolute;
  top: 0; left: 0;
  height: 3px;
  background: hsl(var(--brand-accent));
  /* 宽度由 JS 驱动，transition 由组件动态注入 */
}

.toast-card.failed .toast-progress {
  background: hsl(0 72% 61%);
}

.toast-icon {
  display: grid;
  place-items: center;
  width: 38px; height: 38px;
  border-radius: 8px;
  background: hsl(var(--accent));
  color: hsl(var(--brand-accent));
}

.toast-card.failed .toast-icon {
  color: hsl(0 72% 61%);
}

.toast-body h2 {
  margin: 0 0 6px;
  font-size: 15px;
}

.toast-body p {
  margin: 0;
  color: hsl(var(--muted-foreground));
  font-size: 13px;
  line-height: 1.5;
  overflow-wrap: anywhere;
}

.toast-close {
  border: 0;
  background: transparent;
  color: hsl(var(--muted-foreground));
  cursor: pointer;
  font-size: 20px;
  line-height: 1;
  padding: 0 2px;
}

.toast-close:hover {
  color: hsl(var(--foreground));
}

@keyframes toast-in {
  from { opacity: 0; transform: translateY(-10px); }
  to   { opacity: 1; transform: translateY(0); }
}
```

> **注意**：项目使用 CSS 变量 `hsl(var(--brand-accent))` / `hsl(var(--popover))` 等。在 Electron + Vue3 项目中需要定义对应的 CSS 变量（或在 Tailwind/Unocss 中配置）。

---

## 四、与 React 实现的差异对照

| 方面 | React 现有实现 | Vue3 建议方案 |
|------|---------------|---------------|
| **进度控制** | CSS `animation: toast-progress 4200ms linear forwards`（GPU 合成） | JS `setInterval(100ms)` 驱动 `width` + CSS `transition: width 100ms linear` |
| **悬停暂停** | ❌ 未实现 | ✅ `@mouseenter` 暂停 interval + 禁用 transition |
| **悬停恢复** | ❌ 未实现 | ✅ `@mouseleave` 恢复 interval + 恢复 transition |
| **自动关闭** | `setTimeout(onClose, 4200)` | `remaining <= 0` 时 `emit('close')` |
| **多实例** | ❌ 单例模式（一个 notice 状态） | ✅ Pinia store `toasts[]` 支持多个 |
| **重置机制** | React `key` prop 重建组件 | Vue `:key` 绑定 `toast.id`，每个实例独立 |
| **进度条视觉** | `transform: scaleX(1→0)` | `width: 100% → 0%` + `transition`（JS 驱动更适合暂停/恢复） |

### 为什么用 JS + transition 替代 CSS animation？

CSS `animation` 不支持在暂停后与 JS 定时器保持同步：

- 暂停时 CSS animation 会停在当前帧
- 但 `setTimeout` 的计时还在走
- 恢复后两者就不同步了

改用 `setInterval(100ms)` 递减 `remaining` + `width` 的 `transition` 平滑过渡——暂停时直接把 `transition` 设为 `none`，进度条瞬间停住，恢复时重新加上 `transition`，完美同步。

---

## 五、进阶功能建议

| 功能 | 实现提示 |
|------|---------|
| 多个 toast 堆叠 | store 用 `toasts[]`，CSS 加 `+ .toast-card { margin-top: 8px }` 或 `gap` |
| 点击关闭 | 已实现：关闭按钮 `emit('close')` |
| 点击跳转 | `const emit = defineEmits<{ click: [id: number] }>()`，父组件处理路由 |
| Electron 原生通知 | 通过 `contextBridge` 暴露 API，调用 `Notification API` |
| 拖拽进度条 | 原生 `pointer-events` + `mousemove` 调整 `remaining` |

---

## 六、文件清单（待创建）

| 文件 | 说明 |
|------|------|
| `src/components/ToastNotification.vue` | 通知组件（JS 倒计时 + transition 进度条） |
| `src/stores/notificationStore.ts` | Pinia store（管理多个 toast 实例） |
| `src/assets/css/toast.css` | 通知样式（可合并到全局样式） |
| `src/types/notification.ts` | 类型定义（可选，或直接写在 store 中） |
