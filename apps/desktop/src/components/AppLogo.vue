<script setup lang="ts">
// The MercuryChat mark, drawn inline so its colours come from the theme's
// --logo-* tokens (public/mercurychat.svg stays the fixed brand icon for the
// favicon and the OS). Decorative: hidden from assistive technology.
import { useId } from "vue";

defineProps<{ size: number }>();
// A suffix per instance, so two logos on one page do not share gradient ids.
const uid = useId();
</script>

<template>
  <svg class="logo" :width="size" :height="size" viewBox="0 0 256 256" aria-hidden="true" focusable="false">
    <defs>
      <linearGradient :id="`tile-${uid}`" x2="1" y2="1">
        <stop class="stop" style="stop-color: var(--logo-tile-from)" />
        <stop class="stop" offset="1" style="stop-color: var(--logo-tile-to)" />
      </linearGradient>
      <linearGradient :id="`boot-${uid}`" x2="1" y2="1">
        <stop class="stop boot-from" style="stop-color: var(--logo-boot-from)" />
        <stop class="stop boot-to" offset="1" style="stop-color: var(--logo-boot-to)" />
      </linearGradient>
      <linearGradient :id="`wing-${uid}`" x2="0" y2="1">
        <stop class="stop" style="stop-color: var(--logo-wing-from)" />
        <stop class="stop" offset="1" style="stop-color: var(--logo-wing-to)" />
      </linearGradient>
    </defs>
    <rect x="6" y="6" width="244" height="244" rx="57" :fill="`url(#tile-${uid})`" style="stroke: var(--logo-tile-edge)" stroke-width="4" />
    <g transform="translate(10 11) scale(.92)">
      <path
        :fill="`url(#boot-${uid})`"
        d="M108 51h57a5 5 0 0 1 5 6l-10 73a22 22 0 0 0 9 20l21 14h10c12 0 20 8 20 19v15H92a10 10 0 0 1-10-10L94 64a14 14 0 0 1 14-13Z"
      />
      <path d="M102 68h62M93 183h118" fill="none" style="stroke: var(--logo-line)" stroke-width="8" stroke-linecap="round" />
    </g>
    <path
      :fill="`url(#wing-${uid})`"
      style="stroke: var(--logo-wing-line)"
      stroke-width="7"
      stroke-linejoin="round"
      d="M128 139c-22-16-34-54-87-68 4 24 17 47 41 62l-26-5c9 19 27 33 47 37l-20 5c12 13 31 14 46 4 15-10 16-24-1-35Z"
    />
  </svg>
</template>

<style scoped>
.logo { display: block; -webkit-user-drag: none; }
</style>
