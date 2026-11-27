// Serial monitor (xterm.js)
export class Serial {
  constructor(el) {
    this.term = new window.Terminal({
      convertEol: true, fontSize: 12.5, scrollback: 20000, cursorBlink: true,
      fontFamily: '"JetBrains Mono", "Fira Code", Menlo, Consolas, monospace',
      theme: { background: '#111214', foreground: '#d4d6db', cursor: '#4a9eff' },
    });
    this.fitAddon = new window.FitAddon.FitAddon();
    this.term.loadAddon(this.fitAddon);
    this.term.open(el);
    this.onData = null;
    /** plain-text copy of what was printed (last 200 kB; used by tests and "copy") */
    this.text = '';
    this.term.onData((d) => this.onData && this.onData(d));
    setTimeout(() => this.fit(), 50);
  }
  fit() { try { this.fitAddon.fit(); } catch (e) { /* hidden */ } }
  write(b64) {
    const bin = atob(b64);
    const u8 = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) u8[i] = bin.charCodeAt(i);
    this.term.write(u8);
    this.text = (this.text + bin).slice(-200000);
    // MicroPython's REPL only executes on CR: switch the line ending once we see it
    if (!this.sawMicroPython && bin.includes('MicroPython')) {
      this.sawMicroPython = true;
      this.onMicroPython && this.onMicroPython();
    }
  }
  echo(s) { this.term.write(`\x1b[2m> ${s}\x1b[0m\r\n`); }
  clear() { this.term.clear(); this.text = ''; }
}
