import type {Config} from '@docusaurus/types';
import type * as Preset from '@docusaurus/preset-classic';
import {lightTheme, darkTheme} from './src/prismTheme';

const config: Config = {
  title: 'trillium-cli',
  tagline: 'A batteries-included HTTP toolkit',
  url: 'https://cli.trillium.rs',
  baseUrl: '/',
  organizationName: 'trillium-rs',
  projectName: 'trillium-cli',

  onBrokenLinks: 'throw',
  markdown: {
    hooks: {
      onBrokenMarkdownLinks: 'warn',
    },
  },

  i18n: {
    defaultLocale: 'en',
    locales: ['en'],
  },

  presets: [
    [
      'classic',
      {
        docs: {
          path: 'guide',
          routeBasePath: '/',
          sidebarPath: './sidebars.ts',
          editUrl:
            'https://github.com/trillium-rs/trillium-cli/edit/main/docs/',
        },
        blog: false,
        theme: {
          customCss: './src/css/custom.css',
        },
      } satisfies Preset.Options,
    ],
  ],

  themeConfig: {
    colorMode: {
      respectPrefersColorScheme: true,
    },
    navbar: {
      title: 'trillium-cli',
      items: [
        {
          href: 'https://trillium.rs',
          label: 'trillium.rs',
          position: 'right',
        },
        {
          href: 'https://github.com/trillium-rs/trillium-cli',
          label: 'GitHub',
          position: 'right',
        },
      ],
    },
    footer: {
      style: 'dark',
      links: [
        {
          title: 'Learn',
          items: [
            {label: 'Guide', to: '/'},
            {label: 'trillium.rs', href: 'https://trillium.rs'},
          ],
        },
        {
          title: 'Crates',
          items: [
            {label: 'crates.io', href: 'https://crates.io/crates/trillium-cli'},
          ],
        },
        {
          title: 'Community',
          items: [
            {
              label: 'GitHub',
              href: 'https://github.com/trillium-rs/trillium-cli',
            },
          ],
        },
      ],
      copyright: `Copyright © ${new Date().getFullYear()} Jacob Rothstein. Built with Docusaurus.`,
    },
    prism: {
      theme: lightTheme,
      darkTheme: darkTheme,
      additionalLanguages: ['bash', 'toml', 'http', 'apacheconf'],
    },
  } satisfies Preset.ThemeConfig,
};

export default config;
