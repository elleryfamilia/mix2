/**
 * The JSONL protocol between the Ink UI and mix2-core, version 1.
 * Zod schemas validate every event before it reaches application state;
 * unknown event types are surfaced as `unknown` and ignored upstream so a
 * newer core never crashes an older UI.
 */
import { z } from 'zod';

export const PROTOCOL_VERSION = 1;

const agentKind = z.enum(['claude', 'codex']);
const agentRole = z.enum(['lead', 'teammate']);
const speaker = z.enum(['claude', 'codex', 'team']);

const stanceOutcome = z.enum(['chosen', 'deferred', 'dropped']);

export const stanceSchema = z.object({
  agent: agentKind,
  position: z.string(),
  outcome: stanceOutcome,
});

export const disagreementSchema = z.object({
  stances: z.array(stanceSchema),
  resolution: z.string(),
});

export const agentInfoSchema = z.object({
  kind: agentKind,
  name: z.string(),
  version: z.string().optional(),
  available: z.boolean(),
  reason: z.string().optional(),
  authenticated: z.boolean().optional(),
  model: z.string().optional(),
  models: z.array(z.string()).optional(),
});

export const eventSchema = z.discriminatedUnion('type', [
  z.object({
    type: z.literal('ready'),
    protocol: z.number(),
    session_id: z.string(),
    lead: agentInfoSchema,
    teammate: agentInfoSchema,
    cwd: z.string(),
    project: z.boolean().optional(),
  }),
  z.object({ type: z.literal('fatal'), message: z.string() }),
  z.object({ type: z.literal('message.user'), turn_id: z.string(), text: z.string() }),
  z.object({ type: z.literal('turn.started'), turn_id: z.string() }),
  z.object({
    type: z.literal('agent.started'),
    turn_id: z.string(),
    agent: agentKind,
    role: agentRole,
  }),
  z.object({
    type: z.literal('agent.text_delta'),
    turn_id: z.string(),
    agent: agentKind,
    role: agentRole,
    text: z.string(),
  }),
  z.object({
    type: z.literal('agent.tool.started'),
    turn_id: z.string(),
    agent: agentKind,
    role: agentRole,
    name: z.string(),
    detail: z.string().optional(),
  }),
  z.object({
    type: z.literal('agent.tool.finished'),
    turn_id: z.string(),
    agent: agentKind,
    role: agentRole,
    name: z.string(),
  }),
  z.object({
    type: z.literal('consult.started'),
    turn_id: z.string(),
    agent: agentKind,
    index: z.number(),
    max: z.number(),
    prompt: z.string().optional(),
  }),
  z.object({
    type: z.literal('consult.completed'),
    turn_id: z.string(),
    agent: agentKind,
    index: z.number(),
    duration_ms: z.number(),
    text: z.string(),
  }),
  z.object({
    type: z.literal('consult.failed'),
    turn_id: z.string(),
    agent: agentKind,
    index: z.number(),
    message: z.string(),
  }),
  z.object({
    type: z.literal('disagreement.recorded'),
    turn_id: z.string(),
    stances: z.array(stanceSchema),
    resolution: z.string(),
    revision: z.number(),
  }),
  z.object({
    type: z.literal('agent.model'),
    agent: agentKind,
    model: z.string().nullish(),
    source: z.string(),
  }),
  z.object({ type: z.literal('lead.synthesizing'), turn_id: z.string(), agent: agentKind }),
  z.object({
    type: z.literal('message.final'),
    turn_id: z.string(),
    speaker,
    lead: agentKind,
    text: z.string(),
    consultations: z.number(),
    duration_ms: z.number(),
    disagreement: disagreementSchema.optional(),
  }),
  z.object({
    type: z.literal('turn.completed'),
    turn_id: z.string(),
    duration_ms: z.number(),
    consultations: z.number(),
  }),
  z.object({ type: z.literal('turn.cancelled'), turn_id: z.string() }),
  z.object({ type: z.literal('turn.failed'), turn_id: z.string(), message: z.string() }),
  z.object({ type: z.literal('warning'), message: z.string() }),
  z.object({ type: z.literal('error'), message: z.string() }),
]);

export type CoreEvent = z.infer<typeof eventSchema>;
export type AgentInfo = z.infer<typeof agentInfoSchema>;
export type StanceOutcome = z.infer<typeof stanceOutcome>;
export type Stance = z.infer<typeof stanceSchema>;
export type Disagreement = z.infer<typeof disagreementSchema>;

export type Command =
  | { type: 'initialize'; protocol: number; lead?: string; cwd?: string; debug?: boolean }
  | { type: 'submit'; id: string; text: string }
  | { type: 'cancel'; turn_id: string }
  | { type: 'set_model'; agent: string; model: string | null }
  | { type: 'shutdown' };

/** Parse one JSONL line from the core. Returns null for lines that are not
 * valid known events (tolerated by design — logged, never fatal). */
export function parseEventLine(line: string): CoreEvent | null {
  const trimmed = line.trim();
  if (!trimmed) return null;
  let raw: unknown;
  try {
    raw = JSON.parse(trimmed);
  } catch {
    return null;
  }
  const result = eventSchema.safeParse(raw);
  return result.success ? result.data : null;
}
