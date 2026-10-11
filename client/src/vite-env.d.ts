/// <reference types="vite/client" />

// `import.meta.env.VITE_*` 的类型来自 vite/client。
// 没有这行，`import.meta.env` 在 vue-tsc 下是不存在的属性。
