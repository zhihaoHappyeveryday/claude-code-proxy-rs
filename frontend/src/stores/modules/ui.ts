// Modified for claude-code-proxy-rs; upstream attribution is in NOTICE.
import { defineStore } from 'pinia'
import { shallowRef } from 'vue'

export const useUiStore = defineStore(
  'ui',
  () => {
    const sidebarCollapsed = shallowRef(false)

    function toggleSidebar(): void {
      sidebarCollapsed.value = !sidebarCollapsed.value
    }

    return {
      sidebarCollapsed,
      toggleSidebar,
    }
  },
  {
    persist: {
      key: 'claude-code-proxy-rs-ui',
      pick: ['sidebarCollapsed'],
    },
  },
)
