/**
 * Semantic color roles and glyph vocabulary for the mix2 TUI.
 *
 * Values come from Design Direction #4 (see docs/design-system.md). UI code
 * must reference roles, never raw hex values. Colors degrade to glyph +
 * label identity on NO_COLOR/16-color terminals (Ink handles downsampling).
 */

export type AgentName = 'claude' | 'codex';
export type SpeakerName = AgentName | 'team';

export const theme = {
  text: {
    primary: '#d6d3dc',
    secondary: '#a9a5b2',
    muted: '#8d8896',
    faint: '#514d59',
  },
  agent: {
    claude: '#e0a06a',
    codex: '#8ab8d6',
    team: '#b795e6',
  },
  chip: {
    appBg: '#d6d3dc',
    appFg: '#17161b',
    claudeFg: '#1c1208',
    codexFg: '#0c141c',
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
  claude: '●',
  codex: '○',
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

export function agentColor(agent: SpeakerName): string {
  return theme.agent[agent];
}

export function agentGlyph(agent: SpeakerName): string {
  return glyphs[agent];
}

export function chipFg(agent: SpeakerName): string {
  return agent === 'claude'
    ? theme.chip.claudeFg
    : agent === 'codex'
      ? theme.chip.codexFg
      : theme.chip.teamFg;
}

export function displayName(agent: SpeakerName): string {
  return agent === 'claude' ? 'Claude' : agent === 'codex' ? 'Codex' : 'Team';
}

/** Maximum comfortable reading width for response text. */
export const MAX_CONTENT_WIDTH = 92;
/** Below this terminal width, consultation tiles stack vertically. */
export const TILE_BREAKPOINT = 88;
