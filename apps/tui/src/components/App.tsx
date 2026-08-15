import { Box, Text, useApp, useInput, useStdout } from 'ink';
import React, { useEffect, useMemo, useReducer, useRef, useState } from 'react';
import type { CoreClient } from '../ipc/client.js';
import type { CoreEvent } from '../ipc/protocol.js';
import { renderConversation } from '../render/conversation.js';
import type { Line } from '../render/lines.js';
import { renderTeamPanel } from '../render/teamPanel.js';
import { initialState, reduce, type AppState } from '../state/store.js';
import { glyphs, spinnerFrames, teamSpinnerFrames, theme } from '../theme/theme.js';
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

export interface AppProps {
  client: CoreClient;
  /** Registers the App's event dispatcher with the client owner. */
  bind: (handlers: { onEvent: (e: CoreEvent) => void; onExit: (code: number | null) => void }) => void;
}

export function App({ client, bind }: AppProps): React.JSX.Element {
  const { exit } = useApp();
  const { stdout } = useStdout();
  const [state, dispatch] = useReducer(reduce, initialState);
  const [editor, setEditor] = useState<EditorState>(emptyEditor);
  const [size, setSize] = useState({ columns: stdout.columns || 100, rows: stdout.rows || 30 });
  const [spinnerIndex, setSpinnerIndex] = useState(0);
  const [scroll, setScroll] = useState<{ top: number; stick: boolean }>({ top: 0, stick: true });
  const turnCounter = useRef(0);
  const ctrlCArmed = useRef(false);

  useEffect(() => {
    bind({
      onEvent: (event) => dispatch({ type: 'core-event', event, now: Date.now() }),
      onExit: (code) => dispatch({ type: 'core-exited', code }),
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

  const lines: Line[] = useMemo(() => {
    const ctx = { width, spinner, teamGlyph, now: Date.now() };
    return state.teamPanelOpen
      ? renderTeamPanel(state, width, ctx.now, teamGlyph)
      : renderConversation(state, ctx);
  }, [state, width, spinner, teamGlyph]);

  const composerRows = composerHeight(editor, width);
  const chromeRows = 2 /* header */ + 1 /* status */ + 1 /* composer separator */;
  const viewportRows = Math.max(3, size.rows - chromeRows - composerRows);

  const maxTop = Math.max(0, lines.length - viewportRows);
  const top = scroll.stick ? maxTop : Math.min(scroll.top, maxTop);
  const visible = lines.slice(top, top + viewportRows);

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

  const runSlashCommand = (text: string) => {
    const command = text.slice(1).split(/\s+/)[0]?.toLowerCase() ?? '';
    setEditor(emptyEditor);
    switch (command) {
      case 'exit':
      case 'quit':
      case 'q':
        quit();
        return;
      case 'help':
        dispatch({
          type: 'local-notice',
          text:
            'commands  /exit quit mix2 · /clear clear the conversation · /team toggle the team panel · /help this list\n' +
            'keys      enter submit · ctrl+j newline · esc cancel · ctrl+t team panel · pgup/pgdn + mouse wheel scroll · ctrl+q quit',
        });
        return;
      case 'clear':
        if (busy) {
          dispatch({ type: 'local-notice', text: '/clear is unavailable while a turn is running' });
        } else {
          dispatch({ type: 'clear-conversation' });
        }
        return;
      case 'team':
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
    if (key.escape) {
      if (state.teamPanelOpen) dispatch({ type: 'close-team-panel' });
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
    return (
      <Box flexDirection="column" width={width} height={size.rows} paddingX={2} paddingY={1}>
        <Text color={theme.status.error} bold>
          mix2 hit a fatal error
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
      <Text> </Text>
      <Box flexDirection="column" flexGrow={1} overflow="hidden">
        {visible.map((line, i) => (
          <LineView key={top + i} line={line} />
        ))}
      </Box>
      <Text> </Text>
      <Composer editor={editor} active={!busy && state.phase === 'ready'} width={width} />
      <StatusBar
        state={state}
        spinner={spinner}
        teamGlyph={teamGlyph}
        width={width}
        scrolledUp={top < maxTop}
        slashOpen={editor.text.startsWith('/')}
      />
    </Box>
  );
}
