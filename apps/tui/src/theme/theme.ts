/**
 * Semantic color roles and glyph vocabulary for the mix2 TUI.
 *
 * Values come from Design Direction #4 (see docs/design-system.md). UI code
 * must reference roles, never raw hex values. Identity is keyed by team slot
 * (`one`/`two`), never by harness: slot one is always the warm mark, slot two
 * the cool one, whichever CLIs back them. Display names come from the
 * session's AgentInfo, not from here. Colors degrade to glyph + label
 * identity on NO_COLOR/16-color terminals (Ink handles downsampling).
 */

export type SlotName = 'one' | 'two';
export type SpeakerName = SlotName | 'team';

export const theme = {
  text: {
    primary: '#d6d3dc',
    secondary: '#a9a5b2',
    muted: '#8d8896',
    faint: '#514d59',
  },
  agent: {
    one: '#e0a06a',
    two: '#8ab8d6',
    team: '#b795e6',
  },
  chip: {
    appBg: '#d6d3dc',
    appFg: '#17161b',
    oneFg: '#1c1208',
    twoFg: '#0c141c',
    teamFg: '#160e20',
  },
  border: {
    subtle: '#2a2830',
    hairline: '#232128',
    bridge: '#39353f',
  },
  status: {
    barBg: '#211f27',
    error: '#e06a6a',
  },
} as const;

export const glyphs = {
  prompt: '❯',
  cursor: '▊',
  one: '●',
  two: '○',
  team: '◐',
  consult: '↔',
  confer: '⇄',
  disagree: '△',
  fail: '×',
  treeMid: '├',
  treeEnd: '└',
  bridge: '╌╌',
  dot: '·',
  check: '✓',
} as const;

export const spinnerFrames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'] as const;

/**
 * The team mark comes alive while the team is thinking: the ◐ half-circle
 * rotates clockwise, then settles back to static ◐ when work completes.
 * Every frame is the same width, so animation never causes layout jitter.
 */
export const teamSpinnerFrames = ['◐', '◓', '◑', '◒'] as const;

export function agentColor(slot: SpeakerName): string {
  return theme.agent[slot];
}

export function agentGlyph(slot: SpeakerName): string {
  return glyphs[slot];
}

export function chipFg(slot: SpeakerName): string {
  return slot === 'one' ? theme.chip.oneFg : slot === 'two' ? theme.chip.twoFg : theme.chip.teamFg;
}

/** Maximum comfortable reading width for response text. */
export const MAX_CONTENT_WIDTH = 92;
/** Below this terminal width, consultation tiles stack vertically. */
export const TILE_BREAKPOINT = 88;
