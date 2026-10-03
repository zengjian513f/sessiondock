<script setup lang="ts">import type {HeaderMetadata} from '../../domain/session-ui/types';defineProps<{item:HeaderMetadata,index:number}>()</script>
<template><span :id="item.id" :class="item.class" :data-node-color="item.color" :data-order="index"><code v-if="item.code">{{item.text}}</code><template v-else>{{item.text}}</template></span></template>
