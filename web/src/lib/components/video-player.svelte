<script lang="ts">
	import api from '$lib/api';
	import { AlertCircleIcon } from '@lucide/svelte/icons';

	export let videoId: number;
	export let pageId: number;

	let loadError = false;

	// 当 videoId 或 pageId 变化时，重置错误状态并重新计算 URL
	$: streamUrl = api.getVideoStreamUrl(videoId, pageId);
	$: if (videoId && pageId) {
		loadError = false;
	}

	function handleError() {
		loadError = true;
	}
</script>

{#if loadError}
	<div
		class="bg-muted/40 flex aspect-video w-full flex-col items-center justify-center gap-2 rounded-lg"
	>
		<AlertCircleIcon class="text-destructive h-8 w-8" />
		<p class="text-muted-foreground text-sm">视频加载失败，可能文件不存在或已被删除</p>
	</div>
{:else}
	<video
		src={streamUrl}
		controls
		autoplay
		playsinline
		class="w-full rounded-lg bg-black"
		style="max-height: 70vh;"
		onerror={handleError}
	></video>
{/if}
