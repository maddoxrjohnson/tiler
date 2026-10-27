import { Composition } from "remotion";
import { Demo, Highlight } from "./Demo";
import { FPS, demoFrames, highlightFrames } from "./timing";

// The recording is 2240x1328 (a 150% display's work area); render at 1280 wide.
const WIDTH = 1280;
const HEIGHT = 760;

export const Root = () => (
  <>
    <Composition id="Demo" component={Demo} durationInFrames={demoFrames} fps={FPS} width={WIDTH} height={HEIGHT} />
    <Composition id="Highlight" component={Highlight} durationInFrames={highlightFrames} fps={FPS} width={WIDTH} height={HEIGHT} />
  </>
);
