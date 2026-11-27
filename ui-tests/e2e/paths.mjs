// Locations shared by the server launcher and the specs.
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { tmpdir } from 'node:os';

const here = dirname(fileURLToPath(import.meta.url));
export const ROOT = join(here, '..', '..');
// outside any git repository (the git tests need a project that is not in one)
export const TMP = join(tmpdir(), `esp32sim-e2e-${process.env.E2E_PORT || '8799'}`);
export const PORT = parseInt(process.env.E2E_PORT || '8799', 10);
export const AI_PORT = parseInt(process.env.E2E_AI_PORT || '8798', 10);
export const BIN = process.env.ESP32SIM_BIN || join(ROOT, 'target', 'release', process.platform === 'win32' ? 'esp32sim.exe' : 'esp32sim');
export const PROJECTS = {
  mock: join(TMP, 'mock'),
  debug: join(TMP, 'debug'),
  created: join(TMP, 'created'),
};
export const FIXTURES = join(ROOT, 'crates', 'esp32sim', 'tests', 'fixtures');
