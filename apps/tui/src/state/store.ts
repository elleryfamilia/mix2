/**
 * Domain state for the mix2 TUI, kept strictly separate from the visual
 * components: a plain reducer over core protocol events plus a handful of
 * local UI actions. Components render this state; they never interpret
 * provider behavior.
 */
import type { AgentInfo, CoreEvent } from '../ipc/protocol.js';
import type { AgentName, SpeakerName } from '../theme/theme.js';

export interface ToolActivity {
  name: string;
  detail?: string;
  done: boolean;
}

export interface ConsultState {
  index: number;
  max: number;
  agent: AgentName;
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

export interface ActiveTurn {
  id: string;
  phase: TurnPhase;
  startedAt: number;
  /** Lead text streamed since the last settle point. */
  streamText: string;
  tools: ToolActivity[];
  toolsCompleted: number;
  consults: ConsultState[];
  leadAgent: AgentName;
}

export type ConversationItem =
  | { kind: 'user'; text: string }
  | { kind: 'interim'; agent: AgentName; text: string }
  | {
      kind: 'activity';
      agent: AgentName;
      toolsCount: number;
      details: string[];
      durationMs: number;
    }
  | {
      kind: 'trace';
      leadAgent: AgentName;
      leadMs: number;
      consultCount: number;
      teammateAgent: AgentName;
      teammateMs: number;
    }
  | {
      kind: 'final';
      speaker: SpeakerName;
      lead: AgentName;
      text: string;
      consultations: number;
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
}

export interface SessionInfo {
  sessionId: string;
  lead: AgentInfo;
  teammate: AgentInfo;
  cwd: string;
}

export interface AppState {
  phase: 'starting' | 'ready' | 'fatal';
  fatalMessage?: string;
  session?: SessionInfo;
  items: ConversationItem[];
  turn?: ActiveTurn;
  lastTurn?: TurnRecord;
  lastSummary?: { durationMs: number; consultations: number };
  teamPanelOpen: boolean;
}

export const initialState: AppState = {
  phase: 'starting',
  items: [],
  teamPanelOpen: false,
};

export type Action =
  | { type: 'core-event'; event: CoreEvent; now: number }
  | { type: 'core-exited'; code: number | null }
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
  return [...items, { kind: 'interim', agent: turn.leadAgent, text }];
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
      { kind: 'activity', agent: turn.leadAgent, toolsCount: turn.toolsCompleted, details, durationMs },
    ];
  }
  const doneConsults = turn.consults.filter((c) => c.status !== 'running');
  if (doneConsults.length > 0) {
    const teammate = doneConsults[0]!.agent;
    const teammateMs = doneConsults.reduce((sum, c) => sum + (c.durationMs ?? 0), 0);
    out = [
      ...out,
      {
        kind: 'trace',
        leadAgent: turn.leadAgent,
        leadMs: durationMs,
        consultCount: doneConsults.length,
        teammateAgent: teammate,
        teammateMs,
      },
    ];
  }
  return out;
}

function recordTurn(turn: ActiveTurn, now: number, outcome: TurnRecord['outcome']): TurnRecord {
  return {
    id: turn.id,
    durationMs: now - turn.startedAt,
    consults: turn.consults,
    toolsCompleted: turn.toolsCompleted,
    outcome,
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
    case 'core-exited':
      if (state.phase === 'fatal') return state;
      return {
        ...state,
        phase: 'fatal',
        fatalMessage: `the mix2 runtime exited unexpectedly (code ${action.code ?? 'unknown'})`,
      };
    case 'core-event':
      return applyEvent(state, action.event, action.now);
  }
}

function applyEvent(state: AppState, event: CoreEvent, now: number): AppState {
  switch (event.type) {
    case 'ready':
      return {
        ...state,
        phase: 'ready',
        session: {
          sessionId: event.session_id,
          lead: event.lead,
          teammate: event.teammate,
          cwd: event.cwd,
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
          leadAgent: (state.session?.lead.kind ?? 'claude') as AgentName,
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
        // Tool use interrupts speech: settle the open stream segment first.
        return {
          ...state,
          items: settleStream(state.items, turn),
          turn: { ...turn, streamText: '', tools: [...turn.tools, tool].slice(-24) },
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
        agent: event.agent,
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
            agent: event.agent,
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
      return {
        ...state,
        items: [
          ...settled,
          {
            kind: 'final',
            speaker: event.speaker,
            lead: event.lead,
            text: event.text,
            consultations: event.consultations,
          },
        ],
        turn,
      };
    }

    case 'turn.completed': {
      const turn = state.turn;
      if (!turn || turn.id !== event.turn_id) return state;
      return {
        ...state,
        turn: undefined,
        lastTurn: recordTurn(turn, now, 'completed'),
        lastSummary: { durationMs: event.duration_ms, consultations: event.consultations },
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

    case 'warning':
    case 'error':
      // Non-fatal diagnostics stay out of the conversation; surface invalid
      // command errors as a notice only if no turn is running.
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
