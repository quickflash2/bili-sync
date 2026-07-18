<script lang="ts">
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import api from '$lib/api';
	import SquareArrowOutUpRightIcon from '@lucide/svelte/icons/square-arrow-out-up-right';
	import type { ApiError, VideoResponse, UpdateVideoStatusRequest } from '$lib/types';
	import BrushCleaningIcon from '@lucide/svelte/icons/brush-cleaning';
	import RotateCcwIcon from '@lucide/svelte/icons/rotate-ccw';
	import SquarePenIcon from '@lucide/svelte/icons/square-pen';
	import PlayIcon from '@lucide/svelte/icons/play';
	import { setBreadcrumb } from '$lib/stores/breadcrumb';
	import { appStateStore, ToQuery } from '$lib/stores/filter';
	import VideoCard from '$lib/components/video-card.svelte';
	import StatusEditor from '$lib/components/status-editor.svelte';
	import VideoPlayer from '$lib/components/video-player.svelte';
	import { toast } from 'svelte-sonner';

	let videoData: VideoResponse | null = null;
	let loading = false;
	let error: string | null = null;
	let resetDialogOpen = false;
	let resetting = false;
	let clearAndResetDialogOpen = false;
	let clearAndResetting = false;
	let statusEditorOpen = false;
	let statusEditorLoading = false;

	// 视频播放器相关状态
	let playerDialogOpen = false;
	let playerVideoId = 0;
	let playerPageId = 0;
	let playerTitle = '';

	// 判断分页的视频内容是否已下载完成（视频内容子任务状态为 STATUS_OK = 7）
	function isVideoContentDownloaded(downloadStatus: number[]): boolean {
		return downloadStatus[1] === 7;
	}

	// 打开视频播放器
	function openPlayer(videoId: number, pageId: number, title: string) {
		playerVideoId = videoId;
		playerPageId = pageId;
		playerTitle = title;
		playerDialogOpen = true;
	}

	async function loadVideoDetail() {
		const videoId = parseInt($page.params.id!);
		if (isNaN(videoId)) {
			error = '无效的视频 ID';
			toast.error('无效的视频 ID');
			return;
		}
		loading = true;
		error = null;
		try {
			const result = await api.getVideo(videoId);
			videoData = result.data;
		} catch (error) {
			console.error('加载视频详情失败：', error);
			toast.error('加载视频详情失败', {
				description: (error as ApiError).message
			});
		} finally {
			loading = false;
		}
	}

	onMount(() => {
		setBreadcrumb([
			{
				label: '视频',
				href: `/${ToQuery($appStateStore)}`
			},
			{ label: '视频详情' }
		]);
	});

	// 监听路由参数变化
	$: if ($page.params.id) {
		loadVideoDetail();
	}

	async function handleStatusEditorSubmit(request: UpdateVideoStatusRequest) {
		if (!videoData) return;

		statusEditorLoading = true;
		try {
			const result = await api.updateVideoStatus(videoData.video.id, request);
			const data = result.data;

			if (data.success) {
				// 更新本地数据
				videoData = {
					video: data.video,
					pages: data.pages
				};
				statusEditorOpen = false;
				toast.success('状态更新成功');
			} else {
				toast.error('状态更新失败');
			}
		} catch (error) {
			console.error('状态更新失败：', error);
			toast.error('状态更新失败', {
				description: (error as ApiError).message
			});
		} finally {
			statusEditorLoading = false;
		}
	}

	async function handleReset(forceReset: boolean) {
		if (!videoData) return;
		try {
			const result = await api.resetVideoStatus(videoData.video.id, { force: forceReset });
			const data = result.data;
			if (data.resetted) {
				videoData = {
					video: data.video,
					pages: data.pages
				};
				toast.success('重置成功');
			} else {
				toast.info('重置无效', {
					description: `视频「${data.video.name}」没有失败的状态，无需重置`
				});
			}
		} catch (error) {
			console.error('重置失败:', error);
			toast.error('重置失败', {
				description: (error as ApiError).message
			});
		}
	}

	async function handleClearAndReset() {
		if (!videoData) return;
		try {
			const result = await api.clearAndResetVideoStatus(videoData.video.id);
			const data = result.data;
			videoData = {
				video: data.video,
				pages: []
			};
			if (data.warning) {
				toast.warning('清空重置成功', {
					description: data.warning
				});
			} else {
				toast.success('清空重置成功', {
					description: `视频「${data.video.name}」已清空重置`
				});
			}
		} catch (error) {
			console.error('清空重置失败：', error);
			toast.error('清空重置失败', {
				description: (error as ApiError).message
			});
		}
	}
</script>

<svelte:head>
	<title>{videoData?.video.name || '视频详情'} - Bili Sync</title>
</svelte:head>

{#if loading}
	<div class="flex items-center justify-center py-12">
		<div class="text-muted-foreground">加载中...</div>
	</div>
{:else if error}
	<div class="flex items-center justify-center py-12">
		<div class="space-y-2 text-center">
			<p class="text-destructive">{error}</p>
			<button
				class="text-muted-foreground hover:text-foreground text-sm transition-colors"
				onclick={() => goto('/')}
			>
				返回首页
			</button>
		</div>
	</div>
{:else if videoData}
	<!-- 视频信息区域 -->
	<section>
		<div class="mb-4 flex items-center justify-between">
			<h2 class="text-xl font-semibold">视频信息</h2>
			<div class="flex gap-2">
				<Button
					size="sm"
					variant="outline"
					class="shrink-0 cursor-pointer "
					onclick={() => (statusEditorOpen = true)}
					disabled={statusEditorLoading}
				>
					<SquarePenIcon class="mr-2 h-4 w-4" />
					编辑状态
				</Button>
				<Button
					size="sm"
					variant="outline"
					class="shrink-0 cursor-pointer "
					onclick={() => (resetDialogOpen = true)}
					disabled={resetting || clearAndResetting}
				>
					<RotateCcwIcon class="mr-2 h-4 w-4 {resetting ? 'animate-spin' : ''}" />
					重置
				</Button>
				<Button
					size="sm"
					variant="outline"
					class="shrink-0 cursor-pointer "
					onclick={() => (clearAndResetDialogOpen = true)}
					disabled={resetting || clearAndResetting}
				>
					<BrushCleaningIcon class="mr-2 h-4 w-4 {clearAndResetting ? 'animate-spin' : ''}" />
					清空重置
				</Button>
				<Button
					size="sm"
					variant="outline"
					class="shrink-0 cursor-pointer "
					onclick={() =>
						window.open(`https://www.bilibili.com/video/${videoData?.video.bvid}/`, '_blank')}
					disabled={statusEditorLoading}
				>
					<SquareArrowOutUpRightIcon class="mr-2 h-4 w-4" />
					在 B 站打开
				</Button>
			</div>
		</div>

		<div style="margin-bottom: 1rem;">
			<VideoCard
				video={videoData.video}
				mode="detail"
				showActions={false}
				taskNames={['视频封面', '视频信息', 'UP 主头像', 'UP 主信息', '分页下载']}
				bind:resetDialogOpen
				bind:resetting
				bind:clearAndResetDialogOpen
				bind:clearAndResetting
				onReset={handleReset}
				onClearAndReset={handleClearAndReset}
			/>
		</div>
	</section>

	<section>
		{#if videoData.pages && videoData.pages.length > 0}
			<div>
				<div class="mb-4 flex items-center justify-between">
					<h2 class="text-xl font-semibold">分页列表</h2>
					<div class="text-muted-foreground text-sm">
						共 {videoData.pages.length} 个分页
					</div>
				</div>

				<div
					class="grid gap-4"
					style="grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));"
				>
					{#each videoData.pages as pageInfo (pageInfo.id)}
						<div class="relative">
							<VideoCard
								video={{
									id: pageInfo.id,
									name: `P${pageInfo.pid}: ${pageInfo.name}`,
									upper_name: '',
									download_status: pageInfo.download_status,
									should_download: videoData.video.should_download,
									valid: videoData.video.valid
								}}
								mode="page"
								showActions={false}
								customTitle="P{pageInfo.pid}: {pageInfo.name}"
								customSubtitle=""
								taskNames={['视频封面', '视频内容', '视频信息', '视频弹幕', '视频字幕']}
							/>
							{#if isVideoContentDownloaded(pageInfo.download_status)}
								<Button
									size="sm"
									variant="secondary"
									class="absolute right-2 top-2 shrink-0 cursor-pointer"
									onclick={() =>
										openPlayer(videoData.video.id, pageInfo.id, `P${pageInfo.pid}: ${pageInfo.name}`)}
								>
									<PlayIcon class="mr-1 h-4 w-4" />
									播放
								</Button>
							{/if}
						</div>
					{/each}
				</div>
			</div>
		{:else}
			<div class="py-12 text-center">
				<div class="space-y-2">
					<p class="text-muted-foreground">暂无分 P 数据</p>
				</div>
			</div>
		{/if}
	</section>

	<!-- 状态编辑器 -->
	{#if videoData}
		<StatusEditor
			bind:open={statusEditorOpen}
			video={videoData.video}
			pages={videoData.pages}
			loading={statusEditorLoading}
			onsubmit={handleStatusEditorSubmit}
		/>
	{/if}
{/if}

<!-- 视频播放器对话框 -->
<Dialog.Root bind:open={playerDialogOpen}>
	<Dialog.Content class="max-w-[90vw]! lg:max-w-[80vw]!">
		<Dialog.Header>
			<Dialog.Title>{playerTitle}</Dialog.Title>
			<Dialog.Description class="sr-only">视频播放器</Dialog.Description>
		</Dialog.Header>
		<div class="mt-2">
			{#if playerDialogOpen}
				<VideoPlayer videoId={playerVideoId} pageId={playerPageId} />
			{/if}
		</div>
	</Dialog.Content>
</Dialog.Root>
