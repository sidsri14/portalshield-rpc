import { spawnSync } from 'node:child_process';
import { mkdir, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';

const root = resolve(import.meta.dirname, '..');
const outputDir = resolve(root, 'demo/output');
const rawVideo = resolve(outputDir, 'portalshield-demo.webm');
const narration = resolve(outputDir, 'portalshield-narration.mp3');
const narratedVideo = resolve(outputDir, 'portalshield-demo-narrated.mp4');
const finalVideo = resolve(outputDir, 'portalshield-demo.mp4');
const subtitles = resolve(outputDir, 'portalshield-demo.srt');
const narrationTextFile = resolve(outputDir, 'portalshield-narration.txt');
const narrationSentences = [
  'PortalShield is a Rust gateway for Portaldot node operations.',
  'This recording is connected to the documented Portaldot mainnet endpoint, not a mock server.',
  'The health probe checks peer state, chain identity, and the latest block.',
  'The gateway accepts traffic only after those checks pass.',
  'Here, a read-only system-chain request passes through the HTTP gateway.',
  'The upstream is the Portaldot mainnet WebSocket node.',
  'Now the same boundary rejects author-insert-key before anything reaches the upstream.',
  'Read-only calls can retry once after specific transient failures. Extrinsic submissions are never replayed.',
  'The gateway exposes Prometheus counters and per-peer request limits.',
  'Node status is credential-redacted.',
  'The repository includes thirty-two tests and a captured mainnet smoke proof.',
  'CI runs on Linux and Windows, with Rust one point eighty-eight and stable.',
];

await mkdir(outputDir, { recursive: true });
await writeFile(narrationTextFile, `${narrationSentences.join('\n\n')}\n`);
runCommand('uvx', [
  '--from', 'edge-tts',
  'edge-tts',
  '--file', narrationTextFile,
  '--voice', 'en-US-AndrewMultilingualNeural',
  '--rate=-8%',
  '--volume=+0%',
  '--pitch=-2Hz',
  '--write-media', narration,
  '--write-subtitles', subtitles,
  '--no-progress',
]);

const spokenDuration = mediaDuration(narration);
const videoDuration = mediaDuration(rawVideo);
const tailDuration = 0.8;
const outputDuration = Math.max(spokenDuration + tailDuration, videoDuration);
const videoPad = Math.max(0, outputDuration - videoDuration);
const audioPad = Math.max(0, outputDuration - spokenDuration);
const fadeStart = Math.max(0, spokenDuration - 0.7);

runFfmpeg([
  '-i', rawVideo,
  '-i', narration,
  '-filter_complex',
  `[0:v]tpad=stop_mode=clone:stop_duration=${videoPad.toFixed(3)}[v];[1:a]afade=t=in:st=0:d=0.25,afade=t=out:st=${fadeStart.toFixed(3)}:d=0.65,apad=pad_dur=${audioPad.toFixed(3)}[a]`,
  '-map', '[v]',
  '-map', '[a]',
  '-c:v', 'libx264',
  '-preset', 'medium',
  '-crf', '20',
  '-pix_fmt', 'yuv420p',
  '-c:a', 'aac',
  '-b:a', '160k',
  '-movflags', '+faststart',
  '-t', outputDuration.toFixed(3),
  narratedVideo,
]);

runFfmpeg([
  '-i', narratedVideo,
  '-vf', "subtitles=demo/output/portalshield-demo.srt:force_style='FontName=Arial,FontSize=16,PrimaryColour=&H00FFFFFF,OutlineColour=&H00101816,BorderStyle=3,BackColour=&H9007100F,Outline=1,Shadow=0,MarginV=24'",
  '-c:v', 'libx264',
  '-preset', 'medium',
  '-crf', '20',
  '-pix_fmt', 'yuv420p',
  '-c:a', 'copy',
  '-movflags', '+faststart',
  finalVideo,
]);

console.log(`Saved final demo to ${finalVideo}`);

function runFfmpeg(args) {
  runCommand('ffmpeg', ['-y', '-loglevel', 'error', ...args]);
}

function runCommand(command, args) {
  const result = spawnSync(command, args, {
    cwd: root,
    encoding: 'utf8',
    stdio: 'inherit',
  });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${command} exited with status ${result.status}`);
}

function mediaDuration(path) {
  const result = spawnSync(
    'ffprobe',
    ['-v', 'error', '-show_entries', 'format=duration', '-of', 'default=nw=1:nk=1', path],
    { cwd: root, encoding: 'utf8' },
  );
  if (result.status !== 0) throw new Error(`ffprobe failed for ${path}`);
  return Number(result.stdout.trim());
}
