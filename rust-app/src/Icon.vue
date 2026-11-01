<script setup lang="ts">
defineProps<{name:string}>();
const paths:Record<string,string>={home:'M3 10 12 3l9 7v10a1 1 0 0 1-1 1h-5v-7H9v7H4a1 1 0 0 1-1-1z',history:'M4 4h16v16H4z M8 8h8 M8 12h8 M8 16h5',upload:'M12 16V3m-5 5 5-5 5 5 M4 14v6h16v-6',settings:'M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8 M9 3h6l1 3 3 1 2 5-2 5-3 1-1 3H9l-1-3-3-1-2-5 2-5 3-1z',arrow:'M5 12h14m-5-5 5 5-5 5',back:'M19 12H5m5-5-5 5 5 5',close:'M6 6l12 12M6 18 18 6',image:'M3 4h18v16H3z M3 16l6-6 5 5 3-3 4 4 M16 8h.01',play:'m8 4 12 8-12 8z',check:'m5 12 4 4L19 6',chevron:'m6 9 6 6 6-6',refresh:'M20 8a8 8 0 1 0 0 8 M20 3v5h-5',folder:'M3 6h7l2 3h9v11H3z',monitor:'M3 4h18v13H3z M8 21h8 M12 17v4',info:'M12 11v6 M12 7h.01 M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18'};
</script>
<template><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path :d="paths[name]||paths.info"/></svg></template>
