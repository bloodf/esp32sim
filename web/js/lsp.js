// Minimal LSP client (clangd over WebSocket) + Arduino API completion fallback.

const LSP_KIND = ['Text', 'Method', 'Function', 'Constructor', 'Field', 'Variable', 'Class', 'Interface', 'Module', 'Property', 'Unit', 'Value', 'Enum', 'Keyword', 'Snippet', 'Color', 'File', 'Reference', 'Folder', 'EnumMember', 'Constant', 'Struct', 'Event', 'Operator', 'TypeParameter'];

export class LspClient {
  constructor(monaco, root, files) {
    this.m = monaco;
    this.root = root;
    this.files = files;
    this.ws = null;
    this.id = 0;
    this.pending = new Map();
    this.ready = false;
    this.disabled = false;
    this.queue = [];
    this.timers = new Map();
  }

  connect() {
    try {
      this.ws = new WebSocket(`${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/lsp`);
    } catch (e) { this.disabled = true; return; }
    this.ws.onmessage = (ev) => {
      const msg = JSON.parse(ev.data);
      if (msg.esp32sim === 'no-clangd') {
        this.disabled = true;
        console.info('clangd not available: using built-in Arduino completions');
        for (const p of this.pending.values()) p.reject('clangd not available');
        this.pending.clear();
        return;
      }
      if (msg.id !== undefined && this.pending.has(msg.id)) { const p = this.pending.get(msg.id); this.pending.delete(msg.id); msg.error ? p.reject(msg.error) : p.resolve(msg.result); return; }
      if (msg.method === 'textDocument/publishDiagnostics') this.diagnostics(msg.params);
      if (msg.id !== undefined && msg.method) this.raw({ jsonrpc: '2.0', id: msg.id, result: null }); // server->client requests
    };
    this.ws.onopen = async () => {
      let r;
      try {
        r = await this.request('initialize', {
        processId: null, rootUri: 'file://' + this.root, workspaceFolders: [{ uri: 'file://' + this.root, name: 'project' }],
        capabilities: {
          textDocument: {
            completion: { completionItem: { snippetSupport: true, documentationFormat: ['markdown', 'plaintext'] } },
            hover: { contentFormat: ['markdown', 'plaintext'] }, signatureHelp: { signatureInformation: { documentationFormat: ['markdown', 'plaintext'] } },
            definition: {}, publishDiagnostics: {}, synchronization: { didSave: true },
          },
        },
        });
      } catch (e) {
        // no clangd (or it did not answer): the built-in completions stay active
        this.disabled = true;
        return;
      }
      this.notify('initialized', {});
      this.ready = true;
      this.caps = r && r.capabilities;
      for (const f of this.queue) this.open(f);
      this.queue = [];
      this.registerProviders();
    };
    this.ws.onclose = () => { this.ready = false; };
  }

  raw(o) { if (this.ws && this.ws.readyState === 1) this.ws.send(JSON.stringify(o)); }
  request(method, params) {
    const id = ++this.id;
    this.raw({ jsonrpc: '2.0', id, method, params });
    return new Promise((resolve, reject) => { this.pending.set(id, { resolve, reject }); setTimeout(() => { if (this.pending.has(id)) { this.pending.delete(id); reject('timeout'); } }, 8000); });
  }
  notify(method, params) { this.raw({ jsonrpc: '2.0', method, params }); }

  isCpp(f) { return f.model.getLanguageId() === 'cpp'; }
  open(f) {
    if (this.disabled || !this.isCpp(f)) return;
    if (!this.ready) { this.queue.push(f); return; }
    this.notify('textDocument/didOpen', { textDocument: { uri: f.model.uri.toString(), languageId: 'cpp', version: f.version, text: f.model.getValue() } });
  }
  change(f) {
    if (this.disabled || !this.ready || !this.isCpp(f)) return;
    clearTimeout(this.timers.get(f.path));
    this.timers.set(f.path, setTimeout(() => {
      f.version++;
      this.notify('textDocument/didChange', { textDocument: { uri: f.model.uri.toString(), version: f.version }, contentChanges: [{ text: f.model.getValue() }] });
    }, 250));
  }
  save(f) { if (this.ready && this.isCpp(f)) this.notify('textDocument/didSave', { textDocument: { uri: f.model.uri.toString() } }); }
  close(f) { if (this.ready && this.isCpp(f)) this.notify('textDocument/didClose', { textDocument: { uri: f.model.uri.toString() } }); }

  diagnostics(p) {
    const model = this.m.editor.getModel(this.m.Uri.parse(p.uri));
    if (!model) return;
    const sev = [null, this.m.MarkerSeverity.Error, this.m.MarkerSeverity.Warning, this.m.MarkerSeverity.Info, this.m.MarkerSeverity.Hint];
    this.m.editor.setModelMarkers(model, 'clangd', p.diagnostics.map((d) => ({
      startLineNumber: d.range.start.line + 1, startColumn: d.range.start.character + 1, endLineNumber: d.range.end.line + 1, endColumn: d.range.end.character + 1,
      message: d.message, severity: sev[d.severity || 1], source: 'clangd',
    })));
  }

  pos(position) { return { line: position.lineNumber - 1, character: position.column - 1 }; }

  registerProviders() {
    const m = this.m;
    const self = this;
    m.languages.registerCompletionItemProvider('cpp', {
      triggerCharacters: ['.', '>', ':', '(', '"', '<'],
      async provideCompletionItems(model, position) {
        if (!self.ready) return { suggestions: [] };
        let r;
        try { r = await self.request('textDocument/completion', { textDocument: { uri: model.uri.toString() }, position: self.pos(position) }); } catch (e) { return { suggestions: [] }; }
        const items = Array.isArray(r) ? r : (r && r.items) || [];
        const word = model.getWordUntilPosition(position);
        const range = new m.Range(position.lineNumber, word.startColumn, position.lineNumber, word.endColumn);
        return {
          incomplete: r && r.isIncomplete,
          suggestions: items.slice(0, 200).map((it) => {
            let rg = range;
            if (it.textEdit && it.textEdit.range) { const q = it.textEdit.range; rg = new m.Range(q.start.line + 1, q.start.character + 1, q.end.line + 1, q.end.character + 1); }
            return {
              label: it.label.trim(), kind: m.languages.CompletionItemKind[LSP_KIND[(it.kind || 1) - 1]] ?? m.languages.CompletionItemKind.Text,
              detail: it.detail, documentation: it.documentation && (it.documentation.value || it.documentation),
              insertText: (it.textEdit && it.textEdit.newText) || it.insertText || it.label, range: rg, sortText: it.sortText, filterText: it.filterText,
              insertTextRules: it.insertTextFormat === 2 ? m.languages.CompletionItemInsertTextRule.InsertAsSnippet : 0,
            };
          }),
        };
      },
    });
    m.languages.registerHoverProvider('cpp', {
      async provideHover(model, position) {
        if (!self.ready) return null;
        try {
          const r = await self.request('textDocument/hover', { textDocument: { uri: model.uri.toString() }, position: self.pos(position) });
          if (!r || !r.contents) return null;
          const v = r.contents.value || (Array.isArray(r.contents) ? r.contents.map((c) => c.value || c).join('\n\n') : String(r.contents));
          return { contents: [{ value: v }] };
        } catch (e) { return null; }
      },
    });
    m.languages.registerSignatureHelpProvider('cpp', {
      signatureHelpTriggerCharacters: ['(', ','],
      async provideSignatureHelp(model, position) {
        if (!self.ready) return null;
        try {
          const r = await self.request('textDocument/signatureHelp', { textDocument: { uri: model.uri.toString() }, position: self.pos(position) });
          if (!r || !r.signatures || !r.signatures.length) return null;
          return { value: { signatures: r.signatures.map((s) => ({ label: s.label, documentation: s.documentation && (s.documentation.value || s.documentation), parameters: (s.parameters || []).map((p) => ({ label: p.label, documentation: p.documentation })) })), activeSignature: r.activeSignature || 0, activeParameter: r.activeParameter || 0 }, dispose() {} };
        } catch (e) { return null; }
      },
    });
    m.languages.registerDefinitionProvider('cpp', {
      async provideDefinition(model, position) {
        if (!self.ready) return null;
        try {
          const r = await self.request('textDocument/definition', { textDocument: { uri: model.uri.toString() }, position: self.pos(position) });
          const locs = Array.isArray(r) ? r : r ? [r] : [];
          return locs.map((l) => ({ uri: m.Uri.parse(l.uri || l.targetUri), range: toRange(m, l.range || l.targetSelectionRange) }));
        } catch (e) { return null; }
      },
    });
  }
}

function toRange(m, r) { return new m.Range(r.start.line + 1, r.start.character + 1, r.end.line + 1, r.end.character + 1); }

// ---------------------------------------------------------------- Arduino API completions
const API = [
  ['pinMode', 'pinMode(${1:pin}, ${2|OUTPUT,INPUT,INPUT_PULLUP,INPUT_PULLDOWN,OUTPUT_OPEN_DRAIN|});', 'void pinMode(uint8_t pin, uint8_t mode)', 'Configure a GPIO as input or output.'],
  ['digitalWrite', 'digitalWrite(${1:pin}, ${2|HIGH,LOW|});', 'void digitalWrite(uint8_t pin, uint8_t val)', 'Set a digital output.'],
  ['digitalRead', 'digitalRead(${1:pin})', 'int digitalRead(uint8_t pin)', 'Read a digital input (HIGH / LOW).'],
  ['analogRead', 'analogRead(${1:pin})', 'uint16_t analogRead(uint8_t pin)', '12-bit ADC reading (0..4095). ADC1: GPIO32-39; ADC2 pins cannot be used with WiFi.'],
  ['analogReadMilliVolts', 'analogReadMilliVolts(${1:pin})', 'uint32_t analogReadMilliVolts(uint8_t pin)', 'Calibrated ADC reading in mV.'],
  ['analogWrite', 'analogWrite(${1:pin}, ${2:value});', 'void analogWrite(uint8_t pin, int value)', 'PWM output (LEDC), 8-bit by default.'],
  ['analogReadResolution', 'analogReadResolution(${1:12});', 'void analogReadResolution(uint8_t bits)', ''],
  ['analogSetAttenuation', 'analogSetAttenuation(${1|ADC_11db,ADC_6db,ADC_2_5db,ADC_0db|});', 'void analogSetAttenuation(adc_attenuation_t)', ''],
  ['delay', 'delay(${1:ms});', 'void delay(uint32_t ms)', 'Block the task for ms milliseconds (lets other tasks run).'],
  ['delayMicroseconds', 'delayMicroseconds(${1:us});', 'void delayMicroseconds(uint32_t us)', 'Busy-wait for microseconds.'],
  ['millis', 'millis()', 'unsigned long millis()', 'Milliseconds since boot.'],
  ['micros', 'micros()', 'unsigned long micros()', 'Microseconds since boot.'],
  ['attachInterrupt', 'attachInterrupt(digitalPinToInterrupt(${1:pin}), ${2:isr}, ${3|RISING,FALLING,CHANGE,ONLOW,ONHIGH|});', 'void attachInterrupt(uint8_t pin, void (*)(void), int mode)', 'Mark the ISR IRAM_ATTR.'],
  ['detachInterrupt', 'detachInterrupt(${1:pin});', 'void detachInterrupt(uint8_t pin)', ''],
  ['tone', 'tone(${1:pin}, ${2:frequency}, ${3:duration});', 'void tone(uint8_t pin, unsigned int frequency, unsigned long duration = 0)', 'Square wave on a pin (passive buzzer).'],
  ['noTone', 'noTone(${1:pin});', 'void noTone(uint8_t pin)', ''],
  ['ledcSetup', 'ledcSetup(${1:channel}, ${2:5000}, ${3:8});', 'double ledcSetup(uint8_t channel, double freq, uint8_t resolution_bits)', 'Arduino-ESP32 2.x LEDC API.'],
  ['ledcAttachPin', 'ledcAttachPin(${1:pin}, ${2:channel});', 'void ledcAttachPin(uint8_t pin, uint8_t channel)', 'Arduino-ESP32 2.x.'],
  ['ledcAttach', 'ledcAttach(${1:pin}, ${2:5000}, ${3:8});', 'bool ledcAttach(uint8_t pin, uint32_t freq, uint8_t resolution)', 'Arduino-ESP32 3.x.'],
  ['ledcWrite', 'ledcWrite(${1:channel_or_pin}, ${2:duty});', 'void ledcWrite(uint8_t chan, uint32_t duty)', ''],
  ['ledcWriteTone', 'ledcWriteTone(${1:channel}, ${2:freq});', 'double ledcWriteTone(uint8_t chan, double freq)', ''],
  ['map', 'map(${1:value}, ${2:fromLow}, ${3:fromHigh}, ${4:toLow}, ${5:toHigh})', 'long map(long x, long in_min, long in_max, long out_min, long out_max)', ''],
  ['constrain', 'constrain(${1:x}, ${2:a}, ${3:b})', 'constrain(x, a, b)', ''],
  ['random', 'random(${1:max})', 'long random(long max)', ''],
  ['neopixelWrite', 'neopixelWrite(${1:pin}, ${2:r}, ${3:g}, ${4:b});', 'void neopixelWrite(uint8_t pin, uint8_t r, uint8_t g, uint8_t b)', 'Drive one WS2812 via RMT.'],
  ['touchRead', 'touchRead(${1:pin})', 'touch_value_t touchRead(uint8_t pin)', ''],
  ['Serial.begin', 'Serial.begin(${1:115200});', 'void begin(unsigned long baud)', 'Start the UART0 serial port (USB).'],
  ['Serial.println', 'Serial.println(${1});', 'size_t println(...)', ''],
  ['Serial.print', 'Serial.print(${1});', 'size_t print(...)', ''],
  ['Serial.printf', 'Serial.printf("${1:%d}\\n", ${2});', 'size_t printf(const char *format, ...)', ''],
  ['Serial.available', 'Serial.available()', 'int available()', ''],
  ['Serial.read', 'Serial.read()', 'int read()', ''],
  ['Serial.readStringUntil', "Serial.readStringUntil('${1:\\n}')", 'String readStringUntil(char terminator)', ''],
  ['Serial2.begin', 'Serial2.begin(${1:9600}, SERIAL_8N1, ${2:16}, ${3:17});', 'void begin(unsigned long baud, uint32_t config, int8_t rxPin, int8_t txPin)', 'UART2 (default RX2=16, TX2=17).'],
  ['Wire.begin', 'Wire.begin(${1:21}, ${2:22});', 'bool begin(int sda, int scl, uint32_t frequency = 0)', 'Start I2C (default SDA=21, SCL=22).'],
  ['Wire.beginTransmission', 'Wire.beginTransmission(${1:0x3C});', 'void beginTransmission(uint16_t address)', ''],
  ['Wire.write', 'Wire.write(${1:data});', 'size_t write(uint8_t)', ''],
  ['Wire.endTransmission', 'Wire.endTransmission()', 'uint8_t endTransmission(bool sendStop = true)', '0 = success, 2 = address NACK.'],
  ['Wire.requestFrom', 'Wire.requestFrom(${1:addr}, ${2:len})', 'uint8_t requestFrom(uint16_t address, uint8_t size)', ''],
  ['Wire.read', 'Wire.read()', 'int read()', ''],
  ['SPI.begin', 'SPI.begin(${1:18}, ${2:19}, ${3:23}, ${4:5});', 'void begin(int8_t sck, int8_t miso, int8_t mosi, int8_t ss)', 'VSPI default pins.'],
  ['SPI.transfer', 'SPI.transfer(${1:data})', 'uint8_t transfer(uint8_t data)', ''],
  ['WiFi.begin', 'WiFi.begin("${1:ssid}", "${2:password}");', 'wl_status_t begin(const char* ssid, const char *passphrase)', 'Note: the emulator has no radio; WiFi will not connect.'],
  ['xTaskCreatePinnedToCore', 'xTaskCreatePinnedToCore(${1:task}, "${2:name}", ${3:4096}, ${4:NULL}, ${5:1}, ${6:NULL}, ${7:1});', 'BaseType_t xTaskCreatePinnedToCore(...)', 'Create a FreeRTOS task on core 0 or 1.'],
  ['vTaskDelay', 'vTaskDelay(pdMS_TO_TICKS(${1:100}));', 'void vTaskDelay(TickType_t ticks)', ''],
  ['esp_restart', 'esp_restart();', 'void esp_restart()', ''],
  ['ESP.getFreeHeap', 'ESP.getFreeHeap()', 'uint32_t getFreeHeap()', ''],
];
const CONSTS = ['HIGH', 'LOW', 'INPUT', 'OUTPUT', 'INPUT_PULLUP', 'INPUT_PULLDOWN', 'OUTPUT_OPEN_DRAIN', 'LED_BUILTIN', 'RISING', 'FALLING', 'CHANGE', 'SDA', 'SCL', 'MOSI', 'MISO', 'SCK', 'SS', 'A0', 'A3', 'A4', 'A5', 'A6', 'A7', 'DAC1', 'DAC2', 'true', 'false', 'IRAM_ATTR', 'uint8_t', 'uint16_t', 'uint32_t', 'int16_t', 'int32_t', 'String', 'bool', 'byte'];

export function arduinoCompletions(m) {
  m.languages.registerCompletionItemProvider('cpp', {
    triggerCharacters: ['.'],
    provideCompletionItems(model, position) {
      // only offer the built-in list when clangd is not producing results for this model
      const markers = m.editor.getModelMarkers({ owner: 'clangd', resource: model.uri });
      const lineText = model.getLineContent(position.lineNumber).slice(0, position.column - 1);
      const word = model.getWordUntilPosition(position);
      const range = new m.Range(position.lineNumber, word.startColumn, position.lineNumber, word.endColumn);
      const obj = (lineText.match(/(\w+)\.\w*$/) || [])[1];
      const out = [];
      for (const [name, ins, detail, doc] of API) {
        const dot = name.indexOf('.');
        if (obj) {
          if (dot < 0 || name.slice(0, dot) !== obj) continue;
          out.push({ label: name.slice(dot + 1), kind: m.languages.CompletionItemKind.Method, insertText: ins.slice(dot + 1), insertTextRules: m.languages.CompletionItemInsertTextRule.InsertAsSnippet, detail, documentation: doc, range });
        } else {
          out.push({ label: name, kind: dot < 0 ? m.languages.CompletionItemKind.Function : m.languages.CompletionItemKind.Method, insertText: ins, insertTextRules: m.languages.CompletionItemInsertTextRule.InsertAsSnippet, detail, documentation: doc, range, sortText: 'z' + name });
        }
      }
      if (!obj) {
        for (const c of CONSTS) out.push({ label: c, kind: m.languages.CompletionItemKind.Constant, insertText: c, range, sortText: 'zz' + c });
        out.push({ label: 'setup/loop', kind: m.languages.CompletionItemKind.Snippet, insertText: '#include <Arduino.h>\n\nvoid setup() {\n  Serial.begin(115200);\n  $0\n}\n\nvoid loop() {\n}\n', insertTextRules: m.languages.CompletionItemInsertTextRule.InsertAsSnippet, documentation: 'Arduino sketch skeleton', range });
      }
      void markers;
      return { suggestions: out };
    },
  });
}
