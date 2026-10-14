import { spawnSync } from 'node:child_process';
import { mkdir, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';

const root = resolve(import.meta.dirname, '..');
const outputDir = resolve(root, 'demo/output');
const rawVideo = resolve(outputDir, 'portalshield-demo.webm');
const narration = resolve(root, 'demo/portalshield-narration.wav');
const narratedVideo = resolve(outputDir, 'portalshield-demo-narrated.mp4');
const finalVideo = resolve(outputDir, 'portalshield-demo.mp4');
const subtitles = resolve(outputDir, 'portalshield-demo.srt');
const narrationSentences = [
  'PortalShield is a Rust gateway that keeps node automation useful without allowing risky RPC calls.',
  'This demo connects to the documented Portaldot mainnet, not a mock. Health checks verify peer state, chain identity, and the latest block before accepting traffic.',
  'A read-only system chain request passes through the local gateway to the mainnet WebSocket node.',
  'Switch the same client to author insert key, and policy blocks it before it reaches upstream.',
  'Retries are asymmetric. Safe reads may retry once after transient failures. Extrinsic submissions are never replayed.',
  'The gateway exposes Prometheus counters, per-peer request limits, and credential-redacted node status.',
  'The repository includes thirty-two tests and a captured mainnet smoke proof.',
  'Continuous integration runs on Linux and Windows, against Rust one point eighty-eight and stable.',
];
const narrationDurations = [7.375, 11.075, 6.7, 6.25, 8.95, 7.325, 5.325, 6.8];
const sentencePauses = [0.5, 0.5, 1.0, 1.5, 0.4, 0.5, 0.4, 0];

await mkdir(outputDir, { recursive: true });

let captionStart = 0.1;
let captionIndex = 0;
const captionRows = [];
for (let sentenceIndex = 0; sentenceIndex < narrationSentences.length; sentenceIndex += 1) {
  const words = narrationSentences[sentenceIndex].split(/\s+/);
  const chunks = [];
  for (let wordIndex = 0; wordIndex < words.length; wordIndex += 9) {
    chunks.push(words.slice(wordIndex, wordIndex + 9).join(' '));
  }
  for (const chunk of chunks) {
    const chunkDuration = narrationDurations[sentenceIndex] * (chunk.split(/\s+/).length / words.length);
    const captionEnd = captionStart + chunkDuration;
    captionIndex += 1;
    captionRows.push(`${captionIndex}\n${srtTime(captionStart)} --> ${srtTime(captionEnd)}\n${chunk}`);
    captionStart = captionEnd;
  }
  captionStart += sentencePauses[sentenceIndex];
}
await writeFile(subtitles, `${captionRows.join('\n\n')}\n`);

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
  `[0:v]tpad=stop_mode=clone:stop_duration=${videoPad.toFixed(3)}[v];[1:a]afade=t=in:st=0:d=0.2,afade=t=out:st=${fadeStart.toFixed(3)}:d=0.6,loudnorm=I=-16:TP=-1.5:LRA=11,apad=pad_dur=${audioPad.toFixed(3)}[a]`,
  '-map', '[v]',
  '-map', '[a]',
  '-c:v', 'libx264',
  '-preset', 'medium',
  '-crf', '20',
  '-pix_fmt', 'yuv420p',
  '-c:a', 'aac',
  '-b:a', '128k',
  '-ar', '48000',
  '-movflags', '+faststart',
  '-t', outputDuration.toFixed(3),
  narratedVideo,
]);

runFfmpeg([
  '-i', narratedVideo,
  '-vf', "subtitles=demo/output/portalshield-demo.srt:force_style='FontName=Arial,FontSize=13,PrimaryColour=&H00FFFFFF,OutlineColour=&H00101816,BorderStyle=3,BackColour=&H9007100F,Outline=1,Shadow=0,MarginV=20'",
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

function srtTime(seconds) {
  const millis = Math.max(0, Math.round(seconds * 1000));
  const ms = millis % 1000;
  const totalSeconds = Math.floor(millis / 1000);
  const sec = totalSeconds % 60;
  const totalMinutes = Math.floor(totalSeconds / 60);
  const min = totalMinutes % 60;
  const hour = Math.floor(totalMinutes / 60);
  return `${String(hour).padStart(2, '0')}:${String(min).padStart(2, '0')}:${String(sec).padStart(2, '0')},${String(ms).padStart(3, '0')}`;
}
