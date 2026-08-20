import { Box, Text, useApp, useInput, useStdout } from 'ink';
import React, { useEffect, useMemo, useReducer, useRef, useState } from 'react';
import type { CoreClient } from '../ipc/client.js';
import type { CoreEvent } from '../ipc/protocol.js';
import { renderConversationWithAnchors, type PromptAnchor } from '../render/conversation.js';
import type { Line } from '../render/lines.js';
import {
  modelEntries,
  renderModelPanel,
  PROVIDER_DEFAULT,
  type ModelCursor,
} from '../render/modelPanel.js';
import { renderTeamPanel } from '../render/teamPanel.js';
import type { EventEmitter } from 'node:events';
import type { MouseEvent } from '../mouse/sgr.js';
import {
  extractSelection,
  highlightLines,
  isEmptySelection,
  type Selection,
} from '../render/selection.js';
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
}

/** Screen row (1-based) where the conversation viewport starts:
 * row 1 = header bar, row 2 = spacing. */
const VIEWPORT_TOP_ROW = 3;

export function App({ client, bind, mouse }: AppProps): React.JSX.Element {
  const { exit } = useApp();
  const { stdout } = useStdout();
  const [state, dispatch] = useReducer(reduce, initialState);
  const [editor, setEditor] = useState<EditorState>(emptyEditor);
  const [size, setSize] = useState({ columns: stdout.columns || 100, rows: stdout.rows || 30 });
  const [spinnerIndex, setSpinnerIndex] = useState(0);
  const [scroll, setScroll] = useState<{ top: number; stick: boolean }>({ top: 0, stick: true });
  const [selection, setSelection] = useState<Selection | null>(null);
  const [flash, setFlash] = useState<string | null>(null);
  const [modelPanel, setModelPanel] = useState<ModelCursor | null>(null);
  const turnCounter = useRef(0);
  const ctrlCArmed = useRef(false);
  const flashTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

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
    if (modelPanel && state.session) {
      return { lines: renderModelPanel(state.session, modelPanel, width), anchors: [] };
    }
    if (state.teamPanelOpen) {
      return { lines: renderTeamPanel(state, width, ctx.now, teamGlyph), anchors: [] };
    }
    return renderConversationWithAnchors(state, ctx);
  }, [state, width, spinner, teamGlyph, modelPanel]);

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

  const submit = () => {
    const text = editor.text.trim();
    if (!text) return;
    if (text.startsWith('/')) {
      runSlashCommand(text);
      return;
    }
    if (busy || state.phase !== 'ready') return;
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
      case 'copy':
        copyLastAnswer();
        return;
      case 'model': {
        const [, agent, ...modelParts] = text.slice(1).split(/\s+/);
        const model = modelParts.join(' ').trim();
        if (!agent) {
          setModelPanel({ column: 0, index: 0 });
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
      case 'help':
        dispatch({
          type: 'local-notice',
          text:
            'commands  /exit quit mix2 · /clear clear the conversation · /copy copy the last answer · /model show or set models · /activity toggle the activity panel · /help this list\n' +
            'keys      enter submit · ctrl+j newline · esc cancel · ctrl+t activity · ctrl+y copy answer · pgup/pgdn + mouse wheel scroll · ctrl+q quit',
        });
        return;
      case 'clear':
        if (busy) {
          dispatch({ type: 'local-notice', text: '/clear is unavailable while a turn is running' });
        } else {
          dispatch({ type: 'clear-conversation' });
        }
        return;
      case 'activity':
      case 'team': // legacy alias
        dispatch({ type: 'toggle-team-panel' });
        return;
      default:
        dispatch({
          type: 'local-notice',
          text: `unknown command /${command} — try /help`,
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
      if (modelPanel) setModelPanel(null);
      else if (state.teamPanelOpen) dispatch({ type: 'close-team-panel' });
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
      const entryCount = (column: 0 | 1) => modelEntries(infoFor(column).models ?? []).length;
      // Functional updates: batched key events must each see the latest
      // cursor, not the snapshot from this render.
      if (key.upArrow) {
        setModelPanel((prev) => prev && { ...prev, index: Math.max(0, prev.index - 1) });
      } else if (key.downArrow) {
        setModelPanel(
          (prev) =>
            prev && { ...prev, index: Math.min(entryCount(prev.column) - 1, prev.index + 1) },
        );
      } else if (key.leftArrow || key.rightArrow || key.tab) {
        setModelPanel((prev) => {
          if (!prev) return prev;
          const column: 0 | 1 = prev.column === 0 ? 1 : 0;
          return { column, index: Math.min(prev.index, entryCount(column) - 1) };
        });
      } else if (key.return) {
        setModelPanel((prev) => {
          if (prev) {
            const info = infoFor(prev.column);
            const entry = modelEntries(info.models ?? [])[prev.index];
            if (entry) {
              client.send({
                type: 'set_model',
                slot: info.slot,
                model: entry === PROVIDER_DEFAULT ? null : entry,
              });
            }
          }
          return prev;
        });
      }
      return;
    }

    if (state.teamPanelOpen) {
      if (key.upArrow) setScroll({ top: Math.max(0, top - 1), stick: false });
      if (key.downArrow) setScroll({ top: Math.min(top + 1, maxTop), stick: top + 1 >= maxTop });
      return;
    }

    // Composer editing (always available; submit blocked while busy).
    if (key.return && !key.shift) return submit();
    if (input === '\n' && !key.return) return void setEditor((e) => insertText(e, '\n'));
    if (key.return && key.shift) return void setEditor((e) => insertText(e, '\n'));
    if (key.backspace || (key.delete && !key.meta)) return void setEditor(backspace);
    if (key.delete && key.meta) return void setEditor(deleteForward);
    if (key.leftArrow) return void setEditor(moveLeft);
    if (key.rightArrow) return void setEditor(moveRight);
    // With an empty composer, ↑/↓ scroll the conversation — which also
    // makes the mouse wheel work (the terminal's alternate-scroll mode
    // translates wheel ticks into arrow keys in the alt screen).
    if (key.upArrow) {
      if (editor.text.length === 0) {
        setScroll({ top: Math.max(0, top - 1), stick: false });
      } else {
        setEditor(moveUp);
      }
      return;
    }
    if (key.downArrow) {
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
      setEditor((e) => insertText(e, input));
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
