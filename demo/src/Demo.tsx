import {
  AbsoluteFill,
  OffthreadVideo,
  Sequence,
  interpolate,
  spring,
  staticFile,
  useCurrentFrame,
  useVideoConfig,
} from "remotion";
import { FPS, OUTRO, TITLE, clipFrames, highlightEnd, steps } from "./timing";

const BG = "#18181b";
const FONT = "Segoe UI, Inter, system-ui, sans-serif";

/** How long a shortcut stays on screen after its command runs. */
const SHOW = 2.2;

const Title = () => {
  const frame = useCurrentFrame();
  const opacity = interpolate(frame, [0, 12, TITLE - 10, TITLE], [0, 1, 1, 0], {
    extrapolateRight: "clamp",
  });
  return (
    <AbsoluteFill style={{ background: BG, color: "white", fontFamily: FONT, alignItems: "center", justifyContent: "center", opacity }}>
      <div style={{ fontSize: 96, fontWeight: 700, letterSpacing: -2 }}>tiler</div>
      <div style={{ fontSize: 30, color: "#a1a1aa", marginTop: 12 }}>
        A keyboard-driven tiling window manager for Windows
      </div>
    </AbsoluteFill>
  );
};

const Outro = () => {
  const frame = useCurrentFrame();
  const opacity = interpolate(frame, [0, 12], [0, 1], { extrapolateRight: "clamp" });
  return (
    <AbsoluteFill style={{ background: BG, color: "white", fontFamily: FONT, alignItems: "center", justifyContent: "center", opacity }}>
      <div style={{ fontSize: 64, fontWeight: 700 }}>tiler</div>
      <div style={{ fontSize: 30, color: "#a1a1aa", marginTop: 12 }}>github.com/maddoxrjohnson/tiler</div>
    </AbsoluteFill>
  );
};

/** The key pill and caption for the step that is currently showing, if any. */
const ShortcutOverlay = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const t = frame / fps;
  const step = [...steps].reverse().find((s) => t >= s.at && t < s.at + SHOW);
  if (!step) return null;
  const local = (t - step.at) * fps;
  const enter = spring({ frame: local, fps, config: { damping: 18, stiffness: 180 } });
  const exit = interpolate(local, [SHOW * fps - 10, SHOW * fps], [1, 0], { extrapolateLeft: "clamp" });
  return (
    <AbsoluteFill style={{ justifyContent: "flex-end", alignItems: "center", paddingBottom: 40 }}>
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 18,
          padding: "14px 24px",
          borderRadius: 16,
          background: "rgba(9, 9, 11, 0.86)",
          border: "1px solid rgba(255,255,255,0.12)",
          fontFamily: FONT,
          color: "white",
          opacity: exit,
          transform: `translateY(${(1 - enter) * 30}px)`,
          boxShadow: "0 10px 30px rgba(0,0,0,0.4)",
        }}
      >
        {step.keys && (
          <div style={{ display: "flex", gap: 6 }}>
            {step.keys.split(" ").map((chord) => (
              <span
                key={chord}
                style={{
                  fontFamily: "Cascadia Mono, Consolas, monospace",
                  fontSize: 24,
                  padding: "4px 12px",
                  borderRadius: 8,
                  background: "#3f3f46",
                  border: "1px solid #52525b",
                }}
              >
                {chord}
              </span>
            ))}
          </div>
        )}
        <span style={{ fontSize: 26 }}>{step.caption}</span>
      </div>
    </AbsoluteFill>
  );
};

/** The real screen recording with the shortcut overlay, from `start` for `length` frames. */
const Clip = ({ length }: { length: number }) => (
  <AbsoluteFill style={{ background: BG }}>
    <Sequence durationInFrames={length}>
      <OffthreadVideo src={staticFile("raw.mp4")} muted style={{ width: "100%", height: "100%", objectFit: "contain" }} />
      <ShortcutOverlay />
    </Sequence>
  </AbsoluteFill>
);

export const Demo = () => (
  <AbsoluteFill style={{ background: BG }}>
    <Sequence durationInFrames={TITLE}>
      <Title />
    </Sequence>
    <Sequence from={TITLE} durationInFrames={clipFrames}>
      <Clip length={clipFrames} />
    </Sequence>
    <Sequence from={TITLE + clipFrames}>
      <Outro />
    </Sequence>
  </AbsoluteFill>
);

/** Shorter cut for the README GIF: no title card, ends after workspace switching. */
export const Highlight = () => {
  const length = Math.round(highlightEnd * FPS);
  return (
    <AbsoluteFill style={{ background: BG }}>
      <Sequence durationInFrames={length}>
        <Clip length={length} />
      </Sequence>
      <Sequence from={length}>
        <Outro />
      </Sequence>
    </AbsoluteFill>
  );
};
