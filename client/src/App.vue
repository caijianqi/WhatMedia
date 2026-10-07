<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { health } from './api/client'

const status = ref<string>('连接中…')

onMounted(async () => {
  try {
    const h = await health()
    status.value = `已连接 · v${h.version} · ${h.arch}`
  } catch (e) {
    status.value = `无法连接服务端：${(e as Error).message}`
  }
})
</script>

<template>
  <main>
    <h1>Mosaic</h1>
    <p>{{ status }}</p>
  </main>
</template>

<style scoped>
main { padding: 2rem; font-family: system-ui, sans-serif; }
h1 { font-size: 1.5rem; font-weight: 500; }
p { color: #5f5e5a; }
</style>
