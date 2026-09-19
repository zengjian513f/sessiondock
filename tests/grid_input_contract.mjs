import assert from 'node:assert/strict';
import {test} from 'node:test';
import {InputEncoder, KeyCapture} from '../legacy-web/grid/input.js';

function defaults() {
  return {
    alt: false,
    app_cursor: false,
    app_keypad: false,
    bracketed_paste: false,
    focus_events: false,
    mouse: 'none',
    mouse_encoding: 'default',
  };
}

function key(name, extras = {}) {
  return {
    key: name,
    code: extras.code ?? '',
    ctrlKey: !!extras.ctrlKey,
    altKey: !!extras.altKey,
    shiftKey: !!extras.shiftKey,
    metaKey: !!extras.metaKey,
    isComposing: !!extras.isComposing,
  };
}

function dump(value) {
  if (value == null) return String(value);
  return [...value].map(ch => {
    const c = ch.codePointAt(0);
    if (c < 32 || c === 127) return '\\x' + c.toString(16).padStart(2, '0');
    if (c > 127) return 'U+' + c.toString(16);
    return ch;
  }).join('');
}

const none = {};
const sgrPress = {mouse: 'press_release', mouse_encoding: 'sgr'};
const sgrMotion = {mouse: 'any_motion', mouse_encoding: 'sgr'};
const sgrButtonMotion = {mouse: 'button_motion', mouse_encoding: 'sgr'};
const defPress = {mouse: 'press_release', mouse_encoding: 'default'};
const utf8Press = {mouse: 'press_release', mouse_encoding: 'utf8'};
const mods = (shift = false, alt = false, ctrl = false) => ({shift, alt, ctrl});

const cases = [
  {name: 'letter a', key: key('a'), out: 'a'},
  {name: 'letter z', key: key('z'), out: 'z'},
  {name: 'space', key: key(' '), out: ' '},
  {name: 'Shift+letter', key: key('A', {shiftKey: true}), out: 'A'},
  {name: 'CJK character', key: key('中'), out: '中'},
  {name: 'emoji key', key: key('😀'), out: '😀'},
  {name: 'Enter', key: key('Enter'), out: '\r'},
  {name: 'Enter+Shift still CR', key: key('Enter', {shiftKey: true}), out: '\r'},
  {name: 'Backspace', key: key('Backspace'), out: '\x7f'},
  {name: 'Alt+Backspace', key: key('Backspace', {altKey: true}), out: '\x1b\x7f'},
  {name: 'Ctrl+Backspace', key: key('Backspace', {ctrlKey: true}), out: '\x08'},
  {name: 'Tab', key: key('Tab'), out: '\t'},
  {name: 'Shift+Tab', key: key('Tab', {shiftKey: true}), out: '\x1b[Z'},
  {name: 'Escape', key: key('Escape'), out: '\x1b'},
  {name: 'ArrowUp', key: key('ArrowUp'), out: '\x1b[A'},
  {name: 'ArrowDown', key: key('ArrowDown'), out: '\x1b[B'},
  {name: 'ArrowRight', key: key('ArrowRight'), out: '\x1b[C'},
  {name: 'ArrowLeft', key: key('ArrowLeft'), out: '\x1b[D'},
  {name: 'ArrowUp app_cursor', modes: {app_cursor: true}, key: key('ArrowUp'), out: '\x1bOA'},
  {name: 'ArrowDown app_cursor', modes: {app_cursor: true}, key: key('ArrowDown'), out: '\x1bOB'},
  {name: 'ArrowRight app_cursor', modes: {app_cursor: true}, key: key('ArrowRight'), out: '\x1bOC'},
  {name: 'ArrowLeft app_cursor', modes: {app_cursor: true}, key: key('ArrowLeft'), out: '\x1bOD'},
  {name: 'Ctrl+ArrowUp', key: key('ArrowUp', {ctrlKey: true}), out: '\x1b[1;5A'},
  {name: 'Shift+ArrowRight', key: key('ArrowRight', {shiftKey: true}), out: '\x1b[1;2C'},
  {name: 'Alt+ArrowDown', key: key('ArrowDown', {altKey: true}), out: '\x1b[1;3B'},
  {name: 'Ctrl+Arrow app_cursor still CSI', modes: {app_cursor: true}, key: key('ArrowLeft', {ctrlKey: true}), out: '\x1b[1;5D'},
  {name: 'Home', key: key('Home'), out: '\x1b[H'},
  {name: 'End', key: key('End'), out: '\x1b[F'},
  {name: 'Home app_cursor', modes: {app_cursor: true}, key: key('Home'), out: '\x1bOH'},
  {name: 'End app_cursor', modes: {app_cursor: true}, key: key('End'), out: '\x1bOF'},
  {name: 'Home+Ctrl', key: key('Home', {ctrlKey: true}), out: '\x1b[1;5H'},
  {name: 'End+Shift', key: key('End', {shiftKey: true}), out: '\x1b[1;2F'},
  {name: 'Insert', key: key('Insert'), out: '\x1b[2~'},
  {name: 'Delete', key: key('Delete'), out: '\x1b[3~'},
  {name: 'PageUp', key: key('PageUp'), out: '\x1b[5~'},
  {name: 'PageUp+Shift', key: key('PageUp', {shiftKey: true}), out: '\x1b[5;2~'},
  {name: 'PageDown', key: key('PageDown'), out: '\x1b[6~'},
  {name: 'Delete+Ctrl', key: key('Delete', {ctrlKey: true}), out: '\x1b[3;5~'},
  {name: 'F1', key: key('F1'), out: '\x1bOP'},
  {name: 'F2', key: key('F2'), out: '\x1bOQ'},
  {name: 'F3', key: key('F3'), out: '\x1bOR'},
  {name: 'F4', key: key('F4'), out: '\x1bOS'},
  {name: 'F1+Shift', key: key('F1', {shiftKey: true}), out: '\x1b[1;2P'},
  {name: 'F5', key: key('F5'), out: '\x1b[15~'},
  {name: 'F6', key: key('F6'), out: '\x1b[17~'},
  {name: 'F11', key: key('F11'), out: '\x1b[23~'},
  {name: 'F12', key: key('F12'), out: '\x1b[24~'},
  {name: 'F5+Ctrl', key: key('F5', {ctrlKey: true}), out: '\x1b[15;5~'},
  {name: 'F12+Alt', key: key('F12', {altKey: true}), out: '\x1b[24;3~'},
  {name: 'Ctrl+C', key: key('c', {ctrlKey: true}), out: '\x03'},
  {name: 'Ctrl+A', key: key('a', {ctrlKey: true}), out: '\x01'},
  {name: 'Ctrl+Z', key: key('z', {ctrlKey: true}), out: '\x1a'},
  {name: 'Ctrl+C uppercase', key: key('C', {ctrlKey: true}), out: '\x03'},
  {name: 'Ctrl+Space', key: key(' ', {ctrlKey: true}), out: '\x00'},
  {name: 'Ctrl+[', key: key('[', {ctrlKey: true}), out: '\x1b'},
  {name: 'Ctrl+2', key: key('2', {ctrlKey: true}), out: '\x00'},
  {name: 'Ctrl+@', key: key('@', {ctrlKey: true}), out: '\x00'},
  {name: 'Ctrl+3', key: key('3', {ctrlKey: true}), out: '\x1b'},
  {name: 'Ctrl+\\', key: key('\\', {ctrlKey: true}), out: '\x1c'},
  {name: 'Ctrl+]', key: key(']', {ctrlKey: true}), out: '\x1d'},
  {name: 'Ctrl+^', key: key('^', {ctrlKey: true}), out: '\x1e'},
  {name: 'Ctrl+_', key: key('_', {ctrlKey: true}), out: '\x1f'},
  {name: 'Ctrl+-', key: key('-', {ctrlKey: true}), out: '\x1f'},
  {name: 'Ctrl+?', key: key('?', {ctrlKey: true}), out: '\x7f'},
  {name: 'Ctrl+8', key: key('8', {ctrlKey: true}), out: '\x7f'},
  {name: 'Ctrl+9 unmapped', key: key('9', {ctrlKey: true}), out: null},
  {name: 'Alt+x', key: key('x', {altKey: true}), out: '\x1bx'},
  {name: 'Alt+macOS Option char', key: key('ø', {altKey: true}), out: '\x1bø'},
  {name: 'Meta+c', key: key('c', {metaKey: true}), out: null},
  {name: 'isComposing', key: key('a', {isComposing: true}), out: null},
  {name: 'modifier-only Shift', key: key('Shift', {shiftKey: true}), out: null},
  {name: 'modifier-only Control', key: key('Control', {ctrlKey: true}), out: null},
  {name: 'modifier-only Alt', key: key('Alt', {altKey: true}), out: null},
  {name: 'modifier-only Meta', key: key('Meta', {metaKey: true}), out: null},
  {name: 'modifier-only CapsLock', key: key('CapsLock'), out: null},
  {name: 'Dead', key: key('Dead'), out: null},
  {name: 'ContextMenu named key', key: key('ContextMenu'), out: null},
  {name: 'paste plain', paste: 'hello', out: 'hello'},
  {name: 'paste newlines', paste: 'a\r\nb\nc', out: 'a\rb\rc'},
  {name: 'paste bracketed', modes: {bracketed_paste: true}, paste: 'hi', out: '\x1b[200~hi\x1b[201~'},
  {name: 'paste bracketed newlines', modes: {bracketed_paste: true}, paste: 'x\r\ny\nz', out: '\x1b[200~x\ry\rz\x1b[201~'},
  {name: 'focus in off', focus: true, out: null},
  {name: 'focus out off', focus: false, out: null},
  {name: 'focus in on', modes: {focus_events: true}, focus: true, out: '\x1b[I'},
  {name: 'focus out on', modes: {focus_events: true}, focus: false, out: '\x1b[O'},
  {name: 'mouse none', mouse: ['down', 0, 0, 0, mods()], out: null},
  {name: 'sgr press', modes: sgrPress, mouse: ['down', 0, 0, 0, mods()], out: '\x1b[<0;1;1M'},
  {name: 'sgr release', modes: sgrPress, mouse: ['up', 0, 0, 0, mods()], out: '\x1b[<0;1;1m'},
  {name: 'sgr middle down', modes: sgrPress, mouse: ['down', 1, 2, 3, mods()], out: '\x1b[<1;3;4M'},
  {name: 'sgr motion', modes: sgrMotion, mouse: ['move', 0, 4, 7, mods(true, true, true)], out: '\x1b[<60;5;8M'},
  {name: 'sgr button_motion held', modes: sgrButtonMotion, mouse: ['move', 2, 0, 0, mods()], out: '\x1b[<34;1;1M'},
  {name: 'sgr wheel up + shift', modes: sgrMotion, mouse: ['wheel', 0, 2, 3, mods(true, false, false), -1], out: '\x1b[<68;3;4M'},
  {name: 'sgr wheel down', modes: sgrPress, mouse: ['wheel', 0, 0, 0, mods(), 120], out: '\x1b[<65;1;1M'},
  {name: 'default press', modes: defPress, mouse: ['down', 0, 0, 0, mods()], out: '\x1b[M !!'},
  {name: 'default up', modes: defPress, mouse: ['up', 0, 0, 0, mods()], out: '\x1b[M#!!'},
  {name: 'default up + shift', modes: defPress, mouse: ['up', 0, 0, 0, mods(true)], out: '\x1b[M\'!!'},
  {name: 'default clamp col', modes: defPress, mouse: ['down', 0, 300, 0, mods()], out: `\x1b[M ${String.fromCharCode(255)}!`},
  {name: 'utf8 large column', modes: utf8Press, mouse: ['down', 0, 200, 0, mods()], out: `\x1b[M ${String.fromCodePoint(233)}!`},
  {name: 'move suppressed in press_release', modes: sgrPress, mouse: ['move', 0, 1, 1, mods()], out: null},
  {name: 'wheelAsArrows alt up', modes: {alt: true, mouse: 'none'}, wheelAsArrows: [-1], out: '\x1b[A\x1b[A\x1b[A'},
  {name: 'wheelAsArrows alt down app_cursor', modes: {alt: true, mouse: 'none', app_cursor: true}, wheelAsArrows: [5, 2], out: '\x1bOB\x1bOB'},
  {name: 'wheelAsArrows primary', modes: {alt: false, mouse: 'none'}, wheelAsArrows: [-1], out: null},
  {name: 'wheelAsArrows alt but mouse on', modes: {alt: true, mouse: 'press_release'}, wheelAsArrows: [-1], out: null},
];

test('InputEncoder key/paste/focus/mouse contract', () => {
  assert.equal(typeof KeyCapture, 'function');
  assert.ok(cases.length >= 40, `expected at least 40 cases, got ${cases.length}`);
  const modes = defaults();
  const enc = new InputEncoder(() => modes);
  for (const item of cases) {
    Object.assign(modes, defaults(), item.modes || none);
    let got;
    if (item.key) got = enc.key(item.key);
    else if ('paste' in item) got = enc.paste(item.paste);
    else if ('focus' in item) got = enc.focus(item.focus);
    else if (item.mouse) got = enc.mouse(...item.mouse);
    else if (item.wheelAsArrows) got = enc.wheelAsArrows(...item.wheelAsArrows);
    else throw new Error(`bad case ${item.name}`);
    assert.equal(got, item.out, `${item.name}: got ${dump(got)}, want ${dump(item.out)}`);
  }
});
