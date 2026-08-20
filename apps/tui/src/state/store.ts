/**
 * Domain state for the mix2 TUI, kept strictly separate from the visual
 * components: a plain reducer over core protocol events plus a handful of
 * local UI actions. Components render this state; they never interpret
 * provider behavior.
 */
import type {
  AgentInfo,
  CoreEvent,
  Disagreement,
  DiscoveredHarness,
  Stance,
  TeamProposal,
} from '../ipc/protocol.js';
import type { SlotName, SpeakerName } from '../theme/theme.js';

export interface ToolActivity {
  name: string;
  detail?: string;
  done: boolean;
}

export interface ConsultState {
  index: number;
  max: number;
  slot: SlotName;
  status: 'running' | 'done' | 'failed';
  startedAt: number;
  durationMs?: number;
  /** The lead's written consultation prompt (team panel only). */
  prompt?: string;
  /** Teammate's final consultation response (team panel only). */
  text?: string;
  message?: string;
  tools: ToolActivity[];
  /** Teammate interim text, shown as the last lines inside its tile. */
  streamText: string;
}

export type TurnPhase = 'working' | 'consulting' | 'synthesizing';

/** Live disagreement state for the in-progress turn. Revisioned because the
 * core can send updates concurrently and out of order. */
export interface DisagreementState {
  stances: Stance[];
  resolution: string;
  revision: number;
}

export interface ActiveTurn {
  id: string;
  phase: TurnPhase;
  startedAt: number;
  /** Lead text streamed since the last settle point. */
  streamText: string;
  tools: ToolActivity[];
  toolsCompleted: number;
  consults: ConsultState[];
  leadSlot: SlotName;
  /** Scratchpad files (.mix2/…) the coordinator wrote this turn. */
  scratchpadPaths: string[];
  disagreement?: DisagreementState;
}

export type ConversationItem =
  | { kind: 'user'; text: string }
  | { kind: 'interim'; slot: SlotName; text: string }
  | {
      kind: 'activity';
      slot: SlotName;
      toolsCount: number;
      details: string[];
      durationMs: number;
    }
  | {
      kind: 'trace';
      leadSlot: SlotName;
      leadMs: number;
      consultCount: number;
      teammateSlot: SlotName;
      teammateMs: number;
    }
  | {
      kind: 'final';
      speaker: SpeakerName;
      leadSlot: SlotName;
      text: string;
      consultations: number;
      disagreement?: Disagreement;
    }
  | { kind: 'error'; text: string }
  | { kind: 'cancelled' }
  | { kind: 'notice'; text: string };

export interface TurnRecord {
  id: string;
  durationMs: number;
  consults: ConsultState[];
  toolsCompleted: number;
  outcome: 'completed' | 'cancelled' | 'failed';
  disagreement?: Disagreement;
}

export interface SessionInfo {
  sessionId: string;
  one: AgentInfo;
  two: AgentInfo;
  leadSlot: SlotName;
  cwd: string;
  /** False when the cwd doesn't look like a software project. */
  project: boolean;
}

export function otherSlot(slot: SlotName): SlotName {
  return slot === 'one' ? 'two' : 'one';
}

export function slotInfo(session: SessionInfo, slot: SlotName): AgentInfo {
  return slot === 'one' ? session.one : session.two;
}

export function leadInfo(session: SessionInfo): AgentInfo {
  return slotInfo(session, session.leadSlot);
}

export function teammateInfo(session: SessionInfo): AgentInfo {
  return slotInfo(session, otherSlot(session.leadSlot));
}

/** Display name for a slot ("Claude") or the team. Slots fall back to their
 * ids while the session is still starting. */
export function speakerName(session: SessionInfo | undefined, slot: SpeakerName): string {
  if (slot === 'team') return 'Team';
  if (!session) return slot === 'one' ? 'One' : 'Two';
  return slotInfo(session, slot).name;
}

/** The lowercase register used by tiles, stances, and status lines. */
export function speakerLabel(session: SessionInfo | undefined, slot: SpeakerName): string {
  return speakerName(session, slot).toLowerCase();
}

/** The startup discovery report, kept for the team picker. */
export interface DiscoveryState {
  harnesses: DiscoveredHarness[];
  proposal: TeamProposal;
  /** True when the core auto-confirmed the proposal (no picker needed). */
  auto: boolean;
  /** The core's refusal of the last select_team attempt, shown in place. */
  selectionError?: string;
}

export interface AppState {
  phase: 'starting' | 'selecting-team' | 'ready' | 'fatal';
  fatalMessage?: string;
  session?: SessionInfo;
  discovery?: DiscoveryState;
  items: ConversationItem[];
  turn?: ActiveTurn;
  lastTurn?: TurnRecord;
  lastSummary?: { durationMs: number; consultations: number; disagreements: number };
  teamPanelOpen: boolean;
}

export const initialState: AppState = {
  phase: 'starting',
  items: [],
  teamPanelOpen: false,
};

export type Action =
  | { type: 'core-event'; event: CoreEvent; now: number }
  | { type: 'core-exited'; code: number | null; stderr?: string }
  | { type: 'toggle-team-panel' }
  | { type: 'close-team-panel' }
  /** Local notice from the UI itself (slash command feedback, /help). */
  | { type: 'local-notice'; text: string }
  /** /clear: empty the visible conversation; the session continues. */
  | { type: 'clear-conversation' };

/** Settle the lead's open stream segment into a persistent interim item.
 * Called when activity interrupts the lead's speech (tools, consultation).
 */
function settleStream(items: ConversationItem[], turn: ActiveTurn): ConversationItem[] {
  const text = turn.streamText.trim();
  if (!text) return items;
  return [...items, { kind: 'interim', slot: turn.leadSlot, text }];
}

/** Collapse the live turn into settled conversation items. */
function settleTurn(
  items: ConversationItem[],
  turn: ActiveTurn,
  now: number,
): ConversationItem[] {
  let out = items;
  const durationMs = now - turn.startedAt;
  if (turn.toolsCompleted > 0) {
    const details = turn.tools
      .filter((t) => t.detail)
      .slice(-3)
      .map((t) => t.detail as string);
    out = [
      ...out,
      { kind: 'activity', slot: turn.leadSlot, toolsCount: turn.toolsCompleted, details, durationMs },
    ];
  }
  const doneConsults = turn.consults.filter((c) => c.status !== 'running');
  if (doneConsults.length > 0) {
    const teammate = doneConsults[0]!.slot;
    const teammateMs = doneConsults.reduce((sum, c) => sum + (c.durationMs ?? 0), 0);
    out = [
      ...out,
      {
        kind: 'trace',
        leadSlot: turn.leadSlot,
        leadMs: durationMs,
        consultCount: doneConsults.length,
        teammateSlot: teammate,
        teammateMs,
      },
    ];
  }
  return out;
}

function recordTurn(turn: ActiveTurn, now: number, outcome: TurnRecord['outcome']): TurnRecord {
  // Cancelled and failed turns never carry a disagreement forward — only a
  // completed turn's settled payload is meaningful in history.
  const disagreement: Disagreement | undefined =
    outcome === 'completed' && turn.disagreement
      ? { stances: turn.disagreement.stances, resolution: turn.disagreement.resolution }
      : undefined;
  return {
    id: turn.id,
    durationMs: now - turn.startedAt,
    consults: turn.consults,
    toolsCompleted: turn.toolsCompleted,
    outcome,
    disagreement,
  };
}

export function reduce(state: AppState, action: Action): AppState {
  switch (action.type) {
    case 'toggle-team-panel':
      return { ...state, teamPanelOpen: !state.teamPanelOpen };
    case 'close-team-panel':
      return state.teamPanelOpen ? { ...state, teamPanelOpen: false } : state;
    case 'local-notice':
      return { ...state, items: [...state.items, { kind: 'notice', text: action.text }] };
    case 'clear-conversation':
      return { ...state, items: [], lastSummary: undefined };
    case 'core-exited': {
      if (state.phase === 'fatal') return state;
      // Surface the core's stderr tail — for a binary that can't run at
      // all (missing libc symbols, wrong arch), it's the only clue.
      const detail = action.stderr?.trim() ? `\n\n${action.stderr.trim()}` : '';
      return {
        ...state,
        phase: 'fatal',
        fatalMessage: `the mix2 runtime exited unexpectedly (code ${action.code ?? 'unknown'})${detail}`,
      };
    }
    case 'core-event':
      return applyEvent(state, action.event, action.now);
  }
}

function applyEvent(state: AppState, event: CoreEvent, now: number): AppState {
  switch (event.type) {
    case 'harnesses.discovered':
      return {
        ...state,
        // auto: the core is proceeding on its own; stay in 'starting'.
        phase: event.auto ? state.phase : 'selecting-team',
        discovery: {
          harnesses: event.harnesses,
          proposal: event.proposal,
          auto: event.auto,
        },
      };
    case 'ready':
      return {
        ...state,
        phase: 'ready',
        session: {
          sessionId: event.session_id,
          one: event.one,
          two: event.two,
          leadSlot: event.lead_slot,
          cwd: event.cwd,
          project: event.project ?? true,
        },
      };
    case 'fatal':
      return { ...state, phase: 'fatal', fatalMessage: event.message };
    case 'message.user':
      return {
        ...state,
        items: [...state.items, { kind: 'user', text: event.text }],
        turn: {
          id: event.turn_id,
          phase: 'working',
          startedAt: now,
          streamText: '',
          tools: [],
          toolsCompleted: 0,
          consults: [],
          leadSlot: state.session?.leadSlot ?? 'one',
          scratchpadPaths: [],
        },
        lastSummary: undefined,
      };
    case 'turn.started':
    case 'agent.started':
      return state;

    case 'agent.text_delta': {
      const turn = state.turn;
      if (!turn || turn.id !== event.turn_id) return state;
      if (event.role === 'lead') {
        return { ...state, turn: { ...turn, streamText: turn.streamText + event.text } };
      }
      // Teammate stream feeds the live consultation tile.
      const consults = turn.consults.map((c, i) =>
        i === turn.consults.length - 1 && c.status === 'running'
          ? { ...c, streamText: c.streamText + event.text }
          : c,
      );
      return { ...state, turn: { ...turn, consults } };
    }

    case 'agent.tool.started': {
      const turn = state.turn;
      if (!turn || turn.id !== event.turn_id) return state;
      const tool: ToolActivity = { name: event.name, detail: event.detail, done: false };
      if (event.role === 'lead') {
        // Track scratchpad output so the settled turn can point at it.
        let scratchpadPaths = turn.scratchpadPaths;
        const path = event.detail?.split(/\s+/).find((t) => t.includes('.mix2/'));
        if (path && !scratchpadPaths.includes(path)) {
          scratchpadPaths = [...scratchpadPaths, path];
        }
        // Tool use interrupts speech: settle the open stream segment first.
        return {
          ...state,
          items: settleStream(state.items, turn),
          turn: {
            ...turn,
            streamText: '',
            tools: [...turn.tools, tool].slice(-24),
            scratchpadPaths,
          },
        };
      }
      const consults = turn.consults.map((c, i) =>
        i === turn.consults.length - 1 && c.status === 'running'
          ? { ...c, tools: [...c.tools, tool].slice(-24) }
          : c,
      );
      return { ...state, turn: { ...turn, consults } };
    }

    case 'agent.tool.finished': {
      const turn = state.turn;
      if (!turn || turn.id !== event.turn_id) return state;
      if (event.role === 'lead') {
        const tools = [...turn.tools];
        for (let i = tools.length - 1; i >= 0; i--) {
          const tool = tools[i]!;
          if (!tool.done && tool.name === event.name) {
            tools[i] = { ...tool, done: true };
            break;
          }
        }
        return { ...state, turn: { ...turn, tools, toolsCompleted: turn.toolsCompleted + 1 } };
      }
      const consults = turn.consults.map((c, i) => {
        if (i !== turn.consults.length - 1 || c.status !== 'running') return c;
        const tools = [...c.tools];
        for (let j = tools.length - 1; j >= 0; j--) {
          const tool = tools[j]!;
          if (!tool.done && tool.name === event.name) {
            tools[j] = { ...tool, done: true };
            break;
          }
        }
        return { ...c, tools };
      });
      return { ...state, turn: { ...turn, consults } };
    }

    case 'consult.started': {
      const turn = state.turn;
      if (!turn || turn.id !== event.turn_id) return state;
      const consult: ConsultState = {
        index: event.index,
        max: event.max,
        slot: event.slot,
        status: 'running',
        startedAt: now,
        prompt: event.prompt,
        tools: [],
        streamText: '',
      };
      return {
        ...state,
        items: settleStream(state.items, turn),
        turn: {
          ...turn,
          phase: 'consulting',
          streamText: '',
          consults: [...turn.consults, consult],
        },
      };
    }

    case 'consult.completed': {
      const turn = state.turn;
      if (!turn || turn.id !== event.turn_id) return state;
      const consults = turn.consults.map((c) =>
        c.index === event.index
          ? { ...c, status: 'done' as const, durationMs: event.duration_ms, text: event.text }
          : c,
      );
      return { ...state, turn: { ...turn, consults } };
    }

    case 'consult.failed': {
      const turn = state.turn;
      if (!turn || turn.id !== event.turn_id) return state;
      let found = false;
      let consults = turn.consults.map((c) => {
        if (c.index === event.index) {
          found = true;
          return { ...c, status: 'failed' as const, message: event.message };
        }
        return c;
      });
      if (!found) {
        // A consult can fail before it starts (teammate unavailable).
        consults = [
          ...consults,
          {
            index: event.index,
            max: 0,
            slot: event.slot,
            status: 'failed',
            startedAt: now,
            message: event.message,
            tools: [],
            streamText: '',
          },
        ];
      }
      return { ...state, turn: { ...turn, phase: 'synthesizing', consults } };
    }

    case 'disagreement.recorded': {
      const turn = state.turn;
      if (!turn || turn.id !== event.turn_id) return state;
      // The core can send revisions concurrently; only the latest wins.
      if (event.revision <= (turn.disagreement?.revision ?? 0)) return state;
      return {
        ...state,
        turn: {
          ...turn,
          disagreement: {
            stances: event.stances,
            resolution: event.resolution,
            revision: event.revision,
          },
        },
      };
    }

    case 'lead.synthesizing': {
      const turn = state.turn;
      if (!turn || turn.id !== event.turn_id) return state;
      return { ...state, turn: { ...turn, phase: 'synthesizing' } };
    }

    case 'message.final': {
      const turn = state.turn;
      if (!turn || turn.id !== event.turn_id) return state;
      // The trailing stream segment is the same content as the final
      // message; drop it in favor of the authoritative final text.
      const settled = settleTurn(state.items, turn, now);
      const items: ConversationItem[] = [
        ...settled,
        {
          kind: 'final',
          speaker: event.speaker,
          leadSlot: event.lead_slot,
          text: event.text,
          consultations: event.consultations,
          disagreement: event.disagreement,
        },
      ];
      if (turn.scratchpadPaths.length > 0) {
        items.push({
          kind: 'notice',
          text: `▸ ${turn.scratchpadPaths.join(', ')} updated`,
        });
      }
      // The final payload is authoritative: it overwrites live disagreement
      // state, and an absent payload clears it.
      return {
        ...state,
        items,
        turn: {
          ...turn,
          disagreement: event.disagreement
            ? { ...event.disagreement, revision: turn.disagreement?.revision ?? 1 }
            : undefined,
        },
      };
    }

    case 'turn.completed': {
      const turn = state.turn;
      if (!turn || turn.id !== event.turn_id) return state;
      return {
        ...state,
        turn: undefined,
        lastTurn: recordTurn(turn, now, 'completed'),
        lastSummary: {
          durationMs: event.duration_ms,
          consultations: event.consultations,
          disagreements: turn.disagreement ? 1 : 0,
        },
      };
    }

    case 'turn.cancelled': {
      const turn = state.turn;
      if (!turn || turn.id !== event.turn_id) return state;
      return {
        ...state,
        items: [...settleTurn(settleStream(state.items, turn), turn, now), { kind: 'cancelled' }],
        turn: undefined,
        lastTurn: recordTurn(turn, now, 'cancelled'),
      };
    }

    case 'turn.failed': {
      const turn = state.turn;
      if (!turn || turn.id !== event.turn_id) return state;
      return {
        ...state,
        items: [
          ...settleTurn(settleStream(state.items, turn), turn, now),
          { kind: 'error', text: event.message },
        ],
        turn: undefined,
        lastTurn: recordTurn(turn, now, 'failed'),
      };
    }

    case 'agent.model': {
      const session = state.session;
      if (!session) return state;
      const model = event.model ?? undefined;
      const updateInfo = (info: AgentInfo): AgentInfo =>
        info.slot === event.slot ? { ...info, model } : info;
      const next: AppState = {
        ...state,
        session: {
          ...session,
          one: updateInfo(session.one),
          two: updateInfo(session.two),
        },
      };
      if (event.source === 'selected') {
        const label = speakerLabel(session, event.slot);
        next.items = [
          ...next.items,
          {
            kind: 'notice',
            text: model
              ? `${label} model set to ${model}`
              : `${label} model reset to provider default`,
          },
        ];
      }
      return next;
    }

    case 'error':
      // A refusal while picking is the picker's to display; other errors
      // stay out of the conversation.
      if (state.phase === 'selecting-team' && state.discovery) {
        return {
          ...state,
          discovery: { ...state.discovery, selectionError: event.message },
        };
      }
      return state;

    case 'warning':
      // Non-fatal diagnostics stay out of the conversation.
      return state;
  }
}

/** Elapsed milliseconds formatted as m:ss. */
export function formatElapsed(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  return `${minutes}:${seconds.toString().padStart(2, '0')}`;
}

/** Human summary like "done in 46s" / "done in 2:33". */
export function formatDuration(ms: number): string {
  const seconds = Math.round(ms / 1000);
  if (seconds < 60) return `${seconds}s`;
  return formatElapsed(ms);
}
