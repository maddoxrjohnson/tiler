import data from "./steps.json";

export const FPS = 30;

/** A moment in the recording, logged by scripts/record.ps1 as the command ran. */
export type Step = { at: number; keys: string; caption: string };

export const steps: Step[] = data.steps;

export const TITLE = 2.5 * FPS;
export const OUTRO = 3 * FPS;
export const clipFrames = Math.round(data.clipEnd * FPS);
export const demoFrames = TITLE + clipFrames + OUTRO;

/** The README highlight: the first part of the recording, up to switching workspaces. */
export const highlightEnd = 42;
export const highlightFrames = Math.round(highlightEnd * FPS) + OUTRO;
