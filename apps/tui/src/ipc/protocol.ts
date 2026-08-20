/**
 * The JSONL protocol between the Ink UI and mix2-core, version 3.
 * Participant identity is the slot (`one`/`two`), never the harness name;
 * which CLI backs a slot arrives once in `ready` as display metadata. The
 * startup handshake reports discovered harnesses before `ready`, and the
 * team is settled either automatically or via `select_team`.
 * Zod schemas validate every event before it reaches application state;
 * unknown event types are surfaced as `unknown` and ignored upstream so a
 * newer core never crashes an older UI.
 */
import { z } from 'zod';

export const PROTOCOL_VERSION = 3;

const slotId = z.enum(['one', 'two']);
const harnessKind = z.enum(['claude', 'codex', 'cursor']);
const agentRole = z.enum(['lead', 'teammate']);
const speaker = z.enum(['one', 'two', 'team']);
const authState = z.enum([
  'authenticated',
  'unauthenticated',
  'configured',
  'unsupported',
  'probe_failed',
]);
const capabilityLevel = z.enum(['enforced', 'unverified', 'unsupported']);

const stanceOutcome = z.enum(['chosen', 'deferred', 'dropped']);

export const discoveredHarnessSchema = z.object({
  harness: harnessKind,
  command: z.string(),
  version: z.string().optional(),
  auth: authState,
  available: z.boolean(),
  reason: z.string().optional(),
  note: z.string().optional(),
  lead_eligible: z.boolean(),
  teammate_eligible: z.boolean(),
  capabilities: z.object({
    teammate_read_only: capabilityLevel,
    lead_permission_scoping: capabilityLevel,
    instruction_injection: capabilityLevel,
  }),
});

export const teamProposalSchema = z.object({
  one: harnessKind,
  two: harnessKind,
  lead_slot: slotId,
});

export const stanceSchema = z.object({
  slot: slotId,
  position: z.string(),
  outcome: stanceOutcome,
});

export const disagreementSchema = z.object({
  stances: z.array(stanceSchema),
  resolution: z.string(),
});

export const agentInfoSchema = z.object({
  slot: slotId,
  harness: harnessKind,
  name: z.string(),
  version: z.string().optional(),
  available: z.boolean(),
  reason: z.string().optional(),
  auth: authState,
  model: z.string().optional(),
  models: z.array(z.string()).optional(),
});

export const eventSchema = z.discriminatedUnion('type', [
  z.object({
    type: z.literal('harnesses.discovered'),
    harnesses: z.array(discoveredHarnessSchema),
    proposal: teamProposalSchema,
    auto: z.boolean(),
  }),
  z.object({
    type: z.literal('ready'),
    protocol: z.number(),
    session_id: z.string(),
    one: agentInfoSchema,
    two: agentInfoSchema,
    lead_slot: slotId,
    cwd: z.string(),
    project: z.boolean().optional(),
  }),
  z.object({ type: z.literal('fatal'), message: z.string() }),
  z.object({ type: z.literal('message.user'), turn_id: z.string(), text: z.string() }),
  z.object({ type: z.literal('turn.started'), turn_id: z.string() }),
  z.object({
    type: z.literal('agent.started'),
    turn_id: z.string(),
    slot: slotId,
    role: agentRole,
  }),
  z.object({
    type: z.literal('agent.text_delta'),
    turn_id: z.string(),
    slot: slotId,
    role: agentRole,
    text: z.string(),
  }),
  z.object({
    type: z.literal('agent.tool.started'),
    turn_id: z.string(),
    slot: slotId,
    role: agentRole,
    name: z.string(),
    detail: z.string().optional(),
  }),
  z.object({
    type: z.literal('agent.tool.finished'),
    turn_id: z.string(),
    slot: slotId,
    role: agentRole,
    name: z.string(),
  }),
  z.object({
    type: z.literal('consult.started'),
    turn_id: z.string(),
    slot: slotId,
    index: z.number(),
    max: z.number(),
    prompt: z.string().optional(),
  }),
  z.object({
    type: z.literal('consult.completed'),
    turn_id: z.string(),
    slot: slotId,
    index: z.number(),
    duration_ms: z.number(),
    text: z.string(),
  }),
  z.object({
    type: z.literal('consult.failed'),
    turn_id: z.string(),
    slot: slotId,
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
    slot: slotId,
    model: z.string().nullish(),
    source: z.string(),
  }),
  z.object({ type: z.literal('lead.synthesizing'), turn_id: z.string(), slot: slotId }),
  z.object({
    type: z.literal('message.final'),
    turn_id: z.string(),
    speaker,
    lead_slot: slotId,
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
export type DiscoveredHarness = z.infer<typeof discoveredHarnessSchema>;
export type TeamProposal = z.infer<typeof teamProposalSchema>;

export type Command =
  | {
      type: 'initialize';
      protocol: number;
      lead?: string;
      cwd?: string;
      debug?: boolean;
      interactive?: boolean;
      pick_team?: boolean;
    }
  | { type: 'select_team'; one: string; two: string; lead_slot: string }
  | { type: 'submit'; id: string; text: string }
  | { type: 'cancel'; turn_id: string }
  | { type: 'set_model'; slot: string; model: string | null }
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
