import React, {useEffect, useState} from 'react';
import CodeBlock from '@theme/CodeBlock';
import styles from './styles.module.css';

// The interactive install picker for the docs site. It detects the visitor's
// platform and pre-selects the matching command, but always keeps every option
// one click away — detection is heuristic (`navigator.platform` is deprecated,
// and Apple Silicon under-reports itself), so the user must be able to override.
//
// keep PLATFORMS in sync with the `targets` in ../../../dist-workspace.toml

const REPO = 'trillium-rs/trillium-cli';
const LATEST = `https://github.com/${REPO}/releases/latest/download`;

const SHELL_INSTALL = `curl --proto '=https' --tlsv1.2 -LsSf ${LATEST}/trillium-cli-installer.sh | sh`;
const POWERSHELL_INSTALL = `powershell -c "irm ${LATEST}/trillium-cli-installer.ps1 | iex"`;

type PlatformId = 'mac-arm' | 'mac-intel' | 'linux' | 'windows';

interface Platform {
  id: PlatformId;
  label: string;
  /** Installer one-liner for this platform. */
  command: string;
  /** Language for the rendered code block. */
  lang: 'bash' | 'powershell';
  /** The exact release archive for this target triple. */
  archive: string;
}

const PLATFORMS: Platform[] = [
  {
    id: 'mac-arm',
    label: 'macOS · Apple Silicon',
    command: SHELL_INSTALL,
    lang: 'bash',
    archive: 'trillium-cli-aarch64-apple-darwin.tar.xz',
  },
  {
    id: 'mac-intel',
    label: 'macOS · Intel',
    command: SHELL_INSTALL,
    lang: 'bash',
    archive: 'trillium-cli-x86_64-apple-darwin.tar.xz',
  },
  {
    id: 'linux',
    label: 'Linux · x86_64',
    command: SHELL_INSTALL,
    lang: 'bash',
    archive: 'trillium-cli-x86_64-unknown-linux-gnu.tar.xz',
  },
  {
    id: 'windows',
    label: 'Windows · x86_64',
    command: POWERSHELL_INSTALL,
    lang: 'powershell',
    archive: 'trillium-cli-x86_64-pc-windows-msvc.zip',
  },
];

// The default shown during SSR and before detection runs. macOS Apple Silicon
// is the single most common dev platform for this audience; any visitor on
// something else still sees their option highlighted once the effect runs, and
// can click to switch regardless.
const DEFAULT_PLATFORM: PlatformId = 'mac-arm';

// The only reliable aarch64-vs-x86_64 signal on macOS: the user-agent reports
// Intel even on Apple Silicon, so we probe the GPU renderer string instead.
// Lifted (trimmed) from oranda's artifacts.js. Returns false if unknowable.
function isAppleSilicon(): boolean {
  try {
    const gl = document.createElement('canvas').getContext('webgl');
    const ext = gl?.getExtension('WEBGL_debug_renderer_info');
    const renderer: string =
      (ext && gl?.getParameter(ext.UNMASKED_RENDERER_WEBGL)) || '';
    return /Apple M/.test(renderer) || /Apple GPU/.test(renderer);
  } catch {
    return false;
  }
}

// Returns the best-guess platform, or null when we can't offer a prebuilt
// binary for it (mobile, an unknown OS) — in which case we show every option
// with nothing pre-selected.
function detectPlatform(): PlatformId | null {
  if (typeof navigator === 'undefined') return null;

  const ua = navigator.userAgent || '';
  const appVersion = navigator.appVersion || '';
  const platform = navigator.platform || '';

  // No prebuilt binary for mobile; bail to the full list.
  if (/Android|iPhone|iPad|iPod/.test(ua)) return null;

  if (appVersion.includes('Win') || platform.startsWith('Win')) {
    return 'windows';
  }
  if (appVersion.includes('Mac') || platform.startsWith('Mac')) {
    return isAppleSilicon() ? 'mac-arm' : 'mac-intel';
  }
  if (platform.includes('Linux') || ua.includes('Linux')) {
    return 'linux';
  }
  return null;
}

export default function Install(): React.JSX.Element {
  // `selected` stays null until the user picks or detection lands, so the
  // server render and first client render agree (DEFAULT_PLATFORM) — no
  // hydration mismatch. `autoDetected` drives the "for your system" hint.
  const [selected, setSelected] = useState<PlatformId | null>(null);
  const [autoDetected, setAutoDetected] = useState(false);

  useEffect(() => {
    const detected = detectPlatform();
    if (detected) {
      setSelected(detected);
      setAutoDetected(true);
    }
  }, []);

  const activeId = selected ?? DEFAULT_PLATFORM;
  const active = PLATFORMS.find((p) => p.id === activeId)!;

  return (
    <div className={styles.install}>
      <div className={styles.tabs} role="tablist" aria-label="Platform">
        {PLATFORMS.map((p) => {
          const isActive = p.id === activeId;
          return (
            <button
              key={p.id}
              type="button"
              role="tab"
              aria-selected={isActive}
              className={`button button--sm ${
                isActive ? 'button--primary' : 'button--secondary'
              }`}
              onClick={() => {
                setSelected(p.id);
                setAutoDetected(false);
              }}>
              {p.label}
            </button>
          );
        })}
      </div>

      {autoDetected && (
        <p className={styles.detected}>
          Detected <strong>{active.label}</strong> — here's the command for your
          system. Not right? Pick another above.
        </p>
      )}

      <CodeBlock language={active.lang}>{active.command}</CodeBlock>

      <p className={styles.note}>
        Or download{' '}
        <a href={`${LATEST}/${active.archive}`}>
          <code>{active.archive}</code>
        </a>{' '}
        directly (verify it against{' '}
        <a href={`${LATEST}/sha256.sum`}>
          <code>sha256.sum</code>
        </a>
        ), or — on any platform —{' '}
        <a href="https://github.com/cargo-bins/cargo-binstall">
          <code>cargo binstall trillium-cli</code>
        </a>
        .
      </p>
    </div>
  );
}
