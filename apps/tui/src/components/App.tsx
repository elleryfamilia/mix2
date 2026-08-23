import { Box, Text, useApp, useInput, useStdout } from 'ink';
import React, { useEffect, useMemo, useReducer, useRef, useState } from 'react';
import type { CoreClient } from '../ipc/client.js';
import type { CoreEvent } from '../ipc/protocol.js';
import { renderConversationWithAnchors, type PromptAnchor } from '../render/conversation.js';
import type { Line } from '../render/lines.js';
import {
  filteredModelEntries,
  initialModelSelection,
  modelEntryIndexOf,
  renderModelPanel,
  PROVIDER_DEFAULT,
  type ModelCursor,
  type ModelSelection,
} from '../render/modelPanel.js';
import { renderTeamPanel } from '../render/teamPanel.js';
import {
  entryIndexOf,
  equipSelection,
  initialSelection,
  renderTeamPicker,
  selectable,
  slotEntries,
  type TeamPickerCursor,
  type TeamPickerSelection,
} from '../render/teamPicker.js';
import type { EventEmitter } from 'node:events';
import type { MouseEvent } from '../mouse/sgr.js';
import {
  extractSelection,
  highlightLines,
  isEmptySelection,
  type Selection,
} from '../render/selection.js';
import type { HistoryStore } from '../history.js';
import { initialState, leadInfo, reduce, slotInfo, teammateInfo, type AppState } from '../state/store.js';
import { glyphs, spinnerFrames, teamSpinnerFrames, theme, type SlotName } from '../theme/theme.js';
import { copyToClipboard } from '../util/clipboard.js';
import { Composer, composerHeight } from './Composer.js';
import {
  backspace,
  deleteForward,
  emptyEditor,
  insertText,
  moveDown,
  moveEnd,
  moveHome,
  moveLeft,
  moveRight,
  moveUp,
  type EditorState,
} from './editing.js';
import { Header } from './Header.js';
import { LineView } from './LineView.js';
import { StatusBar } from './StatusBar.js';
import { StickyPrompt } from './StickyPrompt.js';

export interface AppProps {
  client: CoreClient;
  /** Registers the App's event dispatcher with the client owner. */
  bind: (handlers: { onEvent: (e: CoreEvent) => void; onExit: (code: number | null, stderr: string) => void }) => void;
  /** Mouse events from the stdin filter (absent in tests / non-TTY). */
  mouse?: EventEmitter;
  /** Prompt history backing the composer's ↑ recall. Absent (tests),
   * recall still works within the session; nothing touches disk. */
  history?: HistoryStore;
}

/** Screen row (1-based) where the conversation viewport starts:
 * row 1 = header bar, row 2 = spacing. */
const VIEWPORT_TOP_ROW = 3;

export function App({ client, bind, mouse, history }: AppProps): React.JSX.Element {
  const { exit } = useApp();
  const { stdout } = useStdout();
  const [state, dispatch] = useReducer(reduce, initialState);
  const [editor, setEditor] = useState<EditorState>(emptyEditor);
  const [size, setSize] = useState({ columns: stdout.columns || 100, rows: stdout.rows || 30 });
  const [spinnerIndex, setSpinnerIndex] = useState(0);
  const [scroll, setScroll] = useState<{ top: number; stick: boolean }>({ top: 0, stick: true });
  const [selection, setSelection] = useState<Selection | null>(null);
  const [flash, setFlash] = useState<string | null>(null);
  const [modelPanel, setModelPanel] = useState<{
    cursor: ModelCursor;
    selection: ModelSelection;
  } | null>(null);
  const [modelFilter, setModelFilter] = useState('');
  const [picker, setPicker] = useState<{
    cursor: TeamPickerCursor;
    selection: TeamPickerSelection;
  } | null>(null);
  const turnCounter = useRef(0);
  const ctrlCArmed = useRef(false);
  const flashTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  // Prompt history for ↑ recall: loaded once, appended on every submit.
  // While navigating, `index` points into `past` and `draft` holds whatever
  // was being typed when recall began (restored by ↓ past the newest).
  const past = useRef<string[] | undefined>(undefined);
  if (past.current === undefined) past.current = history?.load() ?? [];
  const [histNav, setHistNav] = useState<{ index: number; draft: string } | null>(null);

  useEffect(() => {
    bind({
      onEvent: (event) => dispatch({ type: 'core-event', event, now: Date.now() }),
      onExit: (code, stderr) => dispatch({ type: 'core-exited', code, stderr }),
    });
  }, [bind]);

  useEffect(() => {
    const onResize = () =>
      setSize({ columns: stdout.columns || 100, rows: stdout.rows || 30 });
    stdout.on('resize', onResize);
    return () => {
      stdout.off('resize', onResize);
    };
  }, [stdout]);

  const busy = state.turn !== undefined;

  // Entering the selecting-team phase seeds the picker with the core's
  // proposal (cursor on slot one's proposed harness); leaving it
  // (ready/fatal) clears it.
  useEffect(() => {
    if (state.phase === 'selecting-team' && state.discovery && !picker) {
      const selection = initialSelection(state.discovery);
      setPicker({
        cursor: {
          column: 0,
          index: entryIndexOf(slotEntries(state.discovery, 'one', selection), selection.one),
        },
        selection,
      });
    }
    if (state.phase !== 'selecting-team' && picker) {
      setPicker(null);
    }
  }, [state.phase, state.discovery, picker]);

  // One shared animation clock: the 10fps spinner interval also refreshes
  // elapsed-time displays (each render re-reads Date.now()), so no second
  // timer is needed.
  useEffect(() => {
    if (!busy) return;
    const spin = setInterval(() => setSpinnerIndex((i) => (i + 1) % spinnerFrames.length), 100);
    return () => {
      clearInterval(spin);
    };
  }, [busy]);

  const spinner = spinnerFrames[spinnerIndex] ?? '⠋';
  // The team mark rotates at a calmer pace than the braille spinner
  // (~3 frames per second, full turn ~1.3s) and only while busy.
  const teamGlyph = busy
    ? (teamSpinnerFrames[Math.floor(spinnerIndex / 3) % teamSpinnerFrames.length] ?? glyphs.team)
    : glyphs.team;
  const width = size.columns;

  const { lines, anchors } = useMemo((): { lines: Line[]; anchors: PromptAnchor[] } => {
    const names = state.session
      ? { one: state.session.one.name, two: state.session.two.name }
      : undefined;
    const ctx = { width, spinner, teamGlyph, now: Date.now(), names };
    if (state.phase === 'selecting-team' && state.discovery && picker) {
      return {
        lines: renderTeamPicker(
          state.discovery,
          picker.selection,
          picker.cursor,
          width,
          state.discovery.selectionError,
        ),
        anchors: [],
      };
    }
    if (modelPanel && state.session) {
      return {
        lines: renderModelPanel(
          state.session,
          modelPanel.selection,
          modelPanel.cursor,
          width,
          modelFilter,
        ),
        anchors: [],
      };
    }
    if (state.teamPanelOpen) {
      return { lines: renderTeamPanel(state, width, ctx.now, teamGlyph), anchors: [] };
    }
    return renderConversationWithAnchors(state, ctx);
  }, [state, width, spinner, teamGlyph, modelPanel, modelFilter, picker]);

  const composerRows = composerHeight(editor, width); // includes its frame
  const chromeRows = 2 /* header + spacing */ + 1 /* status */;
  const viewportRows = Math.max(3, size.rows - chromeRows - composerRows);

  const maxTop = Math.max(0, lines.length - viewportRows);
  const top = scroll.stick ? maxTop : Math.min(scroll.top, maxTop);
  const highlighted = highlightLines(lines, selection, width);
  const visible = highlighted.slice(top, top + viewportRows);

  // The prompt governing what's currently on screen, when it has scrolled
  // out of view: last anchor strictly above the viewport top.
  let activeAnchor: PromptAnchor | null = null;
  for (const anchor of anchors) {
    if (anchor.line < top) activeAnchor = anchor;
    else break;
  }

  // Geometry snapshot for the mouse handler (avoids stale closures).
  const geometry = useRef({ top, viewportRows, lines, activeAnchor });
  geometry.current = { top, viewportRows, lines, activeAnchor };

  const showFlash = (text: string) => {
    setFlash(text);
    if (flashTimer.current) clearTimeout(flashTimer.current);
    flashTimer.current = setTimeout(() => setFlash(null), 1800);
  };

  useEffect(() => {
    if (!mouse) return;
    const onMouse = (event: MouseEvent) => {
      const geo = geometry.current;
      if (event.kind === 'wheel-up' || event.kind === 'wheel-down') {
        const delta = event.kind === 'wheel-up' ? -3 : 3;
        setScroll((s) => {
          const max = Math.max(0, geo.lines.length - geo.viewportRows);
          const current = s.stick ? max : Math.min(s.top, max);
          const next = Math.max(0, Math.min(current + delta, max));
          return { top: next, stick: next >= max };
        });
        return;
      }
      const line = geo.top + (event.y - VIEWPORT_TOP_ROW);
      const inViewport =
        event.y >= VIEWPORT_TOP_ROW &&
        event.y < VIEWPORT_TOP_ROW + geo.viewportRows &&
        line < geo.lines.length;
      const pos = { line: Math.max(0, line), col: Math.max(0, event.x - 1) };
      if (event.kind === 'down') {
        // Clicking the sticky prompt bar jumps back to that prompt.
        if (event.y === VIEWPORT_TOP_ROW - 1 && geo.activeAnchor) {
          const target = geo.activeAnchor.line;
          setSelection(null);
          setScroll({ top: target, stick: false });
          return;
        }
        setSelection(inViewport ? { anchor: pos, head: pos } : null);
        return;
      }
      if (event.kind === 'drag') {
        setSelection((sel) => {
          if (!sel) return sel;
          const clampedLine = Math.max(0, Math.min(pos.line, geo.lines.length - 1));
          return { ...sel, head: { line: clampedLine, col: pos.col } };
        });
        return;
      }
      // Release: copy a non-empty selection, exactly like copy-on-select.
      setSelection((sel) => {
        if (!sel) return null;
        if (isEmptySelection(sel)) return null;
        const text = extractSelection(geometry.current.lines, sel);
        if (text.trim().length > 0) {
          copyToClipboard(text);
          showFlash('selection copied');
          return sel; // keep the highlight until the next click
        }
        return null;
      });
    };
    mouse.on('event', onMouse);
    return () => {
      mouse.off('event', onMouse);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mouse]);

  /** Record a submitted input for ↑ recall (consecutive repeats collapse). */
  const remember = (text: string) => {
    setHistNav(null);
    const entries = past.current!;
    if (entries[entries.length - 1] === text) return;
    entries.push(text);
    history?.append(text);
  };

  const submit = () => {
    const text = editor.text.trim();
    if (!text) return;
    if (text.startsWith('/')) {
      remember(text);
      runSlashCommand(text);
      return;
    }
    if (busy || state.phase !== 'ready') return;
    remember(text);
    turnCounter.current += 1;
    client.submit(`t${turnCounter.current}`, text);
    setEditor(emptyEditor);
    setScroll({ top: 0, stick: true });
  };

  const copyLastAnswer = () => {
    for (let i = state.items.length - 1; i >= 0; i--) {
      const item = state.items[i];
      if (item?.kind === 'final') {
        copyToClipboard(item.text);
        dispatch({ type: 'local-notice', text: 'answer copied to clipboard' });
        return;
      }
    }
    dispatch({ type: 'local-notice', text: 'nothing to copy yet' });
  };

  /** Resolve a user-typed participant word to a slot: `one`/`two` always;
   * a harness name or display name while exactly one slot matches it. */
  const resolveSlotWord = (word: string): SlotName | null => {
    if (word === 'one' || word === 'two') return word;
    const session = state.session;
    if (!session) return null;
    const matches = (['one', 'two'] as const).filter((slot) => {
      const info = slotInfo(session, slot);
      return info.harness === word || info.name.toLowerCase() === word;
    });
    return matches.length === 1 ? matches[0]! : null;
  };

  const runSlashCommand = (text: string) => {
    const command = text.slice(1).split(/\s+/)[0]?.toLowerCase() ?? '';
    setEditor(emptyEditor);
    switch (command) {
      case 'exit':
      case 'quit':
      case 'q':
        quit();
        return;
      case 'model': {
        const [, agent, ...modelParts] = text.slice(1).split(/\s+/);
        const model = modelParts.join(' ').trim();
        if (!agent) {
          if (!state.session) return;
          // Seed the pending selection with what's active now; the cursor
          // starts on the lead's equipped entry, like the team picker.
          const selection = initialModelSelection(state.session);
          setModelPanel({
            selection,
            cursor: { column: 0, index: modelEntryIndexOf(leadInfo(state.session), selection, '') },
          });
          setModelFilter('');
          return;
        }
        const slot = resolveSlotWord(agent.toLowerCase());
        if (!slot) {
          dispatch({
            type: 'local-notice',
            text: `unknown agent '${agent}' — /model one <name> or /model two <name> (agent names work too)`,
          });
          return;
        }
        client.send({
          type: 'set_model',
          slot,
          model: !model || model === 'default' ? null : model,
        });
        return;
      }
      case 'clear':
        if (busy) {
          dispatch({ type: 'local-notice', text: '/clear is unavailable while a turn is running' });
        } else {
          dispatch({ type: 'clear-conversation' });
        }
        return;
      case 'team':
        // Switching teams is a fresh session: relaunch the core with the
        // picker forced (was a legacy alias for /activity).
        if (busy) {
          dispatch({ type: 'local-notice', text: '/team is unavailable while a turn is running' });
        } else {
          dispatch({ type: 'reset-session' });
          client.restart();
        }
        return;
      default:
        dispatch({
          type: 'local-notice',
          text: `unknown command /${command} — commands: /model · /team · /clear · /exit`,
        });
    }
  };

  const cancel = () => {
    if (state.turn) client.cancel(state.turn.id);
  };

  const quit = () => {
    client.shutdown();
    exit();
  };

  useInput((input, key) => {
    if (state.phase === 'fatal') {
      if (input === 'q' || key.escape || (key.ctrl && input === 'c')) quit();
      return;
    }

    if (key.ctrl && input === 'q') return quit();

    if (state.phase === 'selecting-team' && state.discovery && picker) {
      const discovery = state.discovery;
      const entriesFor = (column: 0 | 1, selection: TeamPickerSelection) =>
        slotEntries(discovery, column === 0 ? 'one' : 'two', selection);
      const equippedIndex = (column: 0 | 1, selection: TeamPickerSelection) =>
        entryIndexOf(entriesFor(column, selection), column === 0 ? selection.one : selection.two);
      if (key.return) {
        // Enter is pick-equip-advance on the slot columns; the continue
        // button is where the team actually starts. The IPC send stays
        // outside any state updater — updaters may run more than once.
        const { cursor, selection } = picker;
        if (cursor.column === 2) {
          client.selectTeam(selection.one, selection.two, selection.leadSlot);
          return;
        }
        const slot = cursor.column === 0 ? 'one' : 'two';
        const entry = entriesFor(cursor.column, selection)[cursor.index];
        // A disabled entry cannot be equipped; its reason is on screen.
        if (!entry || !selectable(entry, slot, selection.leadSlot)) return;
        // Equipping slot one with slot two's pick swaps them (the helper
        // moves slot two onto the outgoing CLI) rather than duplicating.
        const nextSelection = equipSelection(discovery, selection, slot, entry.harness);
        const column = (cursor.column + 1) as TeamPickerCursor['column'];
        const index = column === 2 ? 0 : equippedIndex(column, nextSelection);
        setPicker({ selection: nextSelection, cursor: { column, index } });
        return;
      }
      if (key.escape) {
        // Esc opts out of picking: the proposal (the defaults) starts.
        const proposal = initialSelection(discovery);
        client.selectTeam(proposal.one, proposal.two, proposal.leadSlot);
        return;
      }
      if (key.leftArrow || key.rightArrow || key.tab) {
        setPicker((prev) => {
          if (!prev) return prev;
          const delta = key.leftArrow ? 2 : 1; // left cycles backwards
          const column = ((prev.cursor.column + delta) % 3) as TeamPickerCursor['column'];
          const index = column === 2 ? 0 : equippedIndex(column, prev.selection);
          return { ...prev, cursor: { column, index } };
        });
        return;
      }
      if (key.upArrow || key.downArrow) {
        setPicker((prev) => {
          if (!prev || prev.cursor.column === 2) return prev;
          // Arrows only move the highlight; equipping is the explicit
          // enter. Disabled entries stay reachable so their reason reads.
          const delta = key.upArrow ? -1 : 1;
          const count = entriesFor(prev.cursor.column, prev.selection).length;
          const index = Math.max(0, Math.min(count - 1, prev.cursor.index + delta));
          return { ...prev, cursor: { ...prev.cursor, index } };
        });
        return;
      }
      if (input.toLowerCase() === 'c' && !key.ctrl && !key.meta) {
        // The coordinator is described, not focused: `c` swaps it from
        // anywhere. The core re-validates eligibility on start.
        setPicker((prev) => {
          if (!prev) return prev;
          const leadSlot = prev.selection.leadSlot === 'one' ? 'two' : 'one';
          return { ...prev, selection: { ...prev.selection, leadSlot } };
        });
        return;
      }
      return;
    }
    if (key.ctrl && input === 'c') {
      if (state.turn) {
        cancel();
        ctrlCArmed.current = false;
      } else if (ctrlCArmed.current) {
        quit();
      } else {
        ctrlCArmed.current = true;
        setTimeout(() => {
          ctrlCArmed.current = false;
        }, 1500);
      }
      return;
    }
    if (key.ctrl && input === 't') {
      dispatch({ type: 'toggle-team-panel' });
      return;
    }
    if (key.ctrl && input === 'y') {
      copyLastAnswer();
      return;
    }
    if (key.escape) {
      if (modelPanel) {
        setModelPanel(null);
        setModelFilter('');
      } else if (state.teamPanelOpen) dispatch({ type: 'close-team-panel' });
      else if (state.turn) cancel();
      return;
    }
    if (key.pageUp) {
      setScroll({ top: Math.max(0, top - (viewportRows - 1)), stick: false });
      return;
    }
    if (key.pageDown) {
      const next = top + (viewportRows - 1);
      setScroll({ top: Math.min(next, maxTop), stick: next >= maxTop });
      return;
    }

    if (modelPanel && state.session) {
      const session = state.session;
      const infoFor = (column: 0 | 1) =>
        column === 0 ? leadInfo(session) : teammateInfo(session);
      const entryCount = (column: 0 | 1) =>
        filteredModelEntries(infoFor(column).models ?? [], modelFilter).length;
      const equippedIndex = (column: 0 | 1 | 2, selection: ModelSelection) =>
        column === 2 ? 0 : modelEntryIndexOf(infoFor(column), selection, modelFilter);
      if (key.return) {
        // Enter is pick-equip-advance on the agent columns; the continue
        // button is where the choices apply, matching the team picker.
        // The IPC sends stay outside any state updater — updaters may run
        // more than once.
        const { cursor, selection } = modelPanel;
        if (cursor.column === 2) {
          for (const info of [session.one, session.two]) {
            const pending = selection[info.slot];
            if (pending !== (info.model ?? null)) {
              client.send({ type: 'set_model', slot: info.slot, model: pending });
            }
          }
          setModelPanel(null);
          setModelFilter('');
          return;
        }
        const info = infoFor(cursor.column);
        const entry = filteredModelEntries(info.models ?? [], modelFilter)[cursor.index];
        if (!entry) return;
        const nextSelection: ModelSelection = {
          ...selection,
          [info.slot]: entry === PROVIDER_DEFAULT ? null : entry,
        };
        const column = (cursor.column + 1) as ModelCursor['column'];
        setModelPanel({
          selection: nextSelection,
          cursor: { column, index: equippedIndex(column, nextSelection) },
        });
        return;
      }
      // Functional updates: batched key events must each see the latest
      // cursor, not the snapshot from this render.
      if (key.upArrow || key.downArrow) {
        setModelPanel((prev) => {
          if (!prev || prev.cursor.column === 2) return prev;
          const delta = key.upArrow ? -1 : 1;
          const index = Math.max(
            0,
            Math.min(entryCount(prev.cursor.column) - 1, prev.cursor.index + delta),
          );
          return { ...prev, cursor: { ...prev.cursor, index } };
        });
      } else if (key.leftArrow || key.rightArrow || key.tab) {
        setModelPanel((prev) => {
          if (!prev) return prev;
          const delta = key.leftArrow ? 2 : 1; // left cycles backwards
          const column = ((prev.cursor.column + delta) % 3) as ModelCursor['column'];
          return { ...prev, cursor: { column, index: equippedIndex(column, prev.selection) } };
        });
      } else if (key.backspace || key.delete) {
        // Narrow the filter; the cursor re-clamps against the wider list.
        setModelFilter((f) => f.slice(0, -1));
        setModelPanel((prev) => prev && { ...prev, cursor: { ...prev.cursor, index: 0 } });
      } else if (input && !key.ctrl && !key.meta) {
        // Type-to-filter: harnesses with long model lists stay navigable.
        setModelFilter((f) => f + input);
        setModelPanel((prev) => prev && { ...prev, cursor: { ...prev.cursor, index: 0 } });
      }
      return;
    }

    if (state.teamPanelOpen) {
      if (key.upArrow) setScroll({ top: Math.max(0, top - 1), stick: false });
      if (key.downArrow) setScroll({ top: Math.min(top + 1, maxTop), stick: top + 1 >= maxTop });
      return;
    }

    // Composer editing (always available; submit blocked while busy).
    // Any edit leaves history-recall mode; plain cursor movement keeps it.
    const edit = (fn: (e: EditorState) => EditorState) => {
      setHistNav(null);
      setEditor(fn);
    };
    /** Put a history entry in the composer, cursor at the end. */
    const recallAt = (index: number, draft: string) => {
      const text = past.current![index]!;
      setHistNav({ index, draft });
      setEditor({ text, cursor: text.length });
    };
    if (key.return && !key.shift) return submit();
    if (input === '\n' && !key.return) return void edit((e) => insertText(e, '\n'));
    if (key.return && key.shift) return void edit((e) => insertText(e, '\n'));
    if (key.backspace || (key.delete && !key.meta)) return void edit(backspace);
    if (key.delete && key.meta) return void edit(deleteForward);
    if (key.leftArrow) return void setEditor(moveLeft);
    if (key.rightArrow) return void setEditor(moveRight);
    // ↑ recalls submitted prompts (on an empty composer, or stepping
    // further back while already recalling); inside typed text it moves
    // the cursor. With no history to offer, an empty-composer ↑/↓ still
    // scrolls the conversation — which also keeps the mouse wheel working
    // where the terminal's alternate-scroll mode translates wheel ticks
    // into arrow keys in the alt screen.
    if (key.upArrow) {
      const entries = past.current!;
      if (histNav) {
        if (histNav.index > 0) recallAt(histNav.index - 1, histNav.draft);
        return;
      }
      if (editor.text.length === 0 && entries.length > 0) {
        recallAt(entries.length - 1, editor.text);
      } else if (editor.text.length === 0) {
        setScroll({ top: Math.max(0, top - 1), stick: false });
      } else {
        setEditor(moveUp);
      }
      return;
    }
    if (key.downArrow) {
      const entries = past.current!;
      if (histNav) {
        // Forward through history; past the newest, the draft comes back.
        if (histNav.index < entries.length - 1) {
          recallAt(histNav.index + 1, histNav.draft);
        } else {
          setHistNav(null);
          setEditor({ text: histNav.draft, cursor: histNav.draft.length });
        }
        return;
      }
      if (editor.text.length === 0) {
        const next = Math.min(top + 1, maxTop);
        setScroll({ top: next, stick: next >= maxTop });
      } else {
        setEditor(moveDown);
      }
      return;
    }
    if (key.ctrl && input === 'a') return void setEditor(moveHome);
    if (key.ctrl && input === 'e') return void setEditor(moveEnd);
    if (key.ctrl || key.meta) return;
    if (input) {
      edit((e) => insertText(e, input));
      setScroll((s) => ({ ...s, stick: true }));
    }
  });

  if (state.phase === 'fatal') {
    // Preflight refusals (agent not installed / signed out) are setup
    // problems, not crashes — don't greet a new user with "fatal error".
    const isPreflight = state.fatalMessage?.startsWith('mix2 needs');
    return (
      <Box flexDirection="column" width={width} height={size.rows} paddingX={2} paddingY={1}>
        <Text color={theme.status.error} bold>
          {isPreflight ? 'mix2 is not ready to start' : 'mix2 hit a fatal error'}
        </Text>
        <Text> </Text>
        <Text color={theme.text.primary}>{state.fatalMessage}</Text>
        <Text> </Text>
        <Text color={theme.text.faint}>press q to quit</Text>
      </Box>
    );
  }

  return (
    <Box flexDirection="column" width={width} height={size.rows}>
      <Header session={state.session} width={width} />
      {activeAnchor ? (
        <StickyPrompt text={activeAnchor.text} width={width} />
      ) : (
        <Text> </Text>
      )}
      <Box flexDirection="column" flexGrow={1} overflow="hidden">
        {visible.map((line, i) => (
          <LineView key={top + i} line={line} />
        ))}
      </Box>
      <Composer editor={editor} active={!busy && state.phase === 'ready'} width={width} />
      <StatusBar
        state={state}
        spinner={spinner}
        teamGlyph={teamGlyph}
        width={width}
        scrolledUp={top < maxTop}
        slashOpen={editor.text.startsWith('/')}
        flash={flash}
        modelPanelOpen={modelPanel !== null}
      />
    </Box>
  );
}
