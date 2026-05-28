import siteConfig from '@generated/docusaurus.config';
import type * as PrismNamespace from 'prismjs';
import type {Optional} from 'utility-types';

// Swizzled (ejected) from @docusaurus/theme-classic to register a custom Prism
// grammar that isn't published in the bundled Prism yet. Everything here is the
// stock implementation except the marked KDL block. See src/prism/prism-kdl.js.
export default function prismIncludeLanguages(
  PrismObject: typeof PrismNamespace,
): void {
  const {
    themeConfig: {prism},
  } = siteConfig;
  const {additionalLanguages} = prism as {additionalLanguages: string[]};

  // Prism components work on the Prism instance on the window, while prism-
  // react-renderer uses its own Prism instance. We temporarily mount the
  // instance onto window, import components to enhance it, then remove it to
  // avoid polluting global namespace.
  // You can mutate PrismObject: registering plugins, deleting languages... As
  // long as you don't re-assign it

  const PrismBefore = globalThis.Prism;
  globalThis.Prism = PrismObject;

  additionalLanguages.forEach((lang) => {
    if (lang === 'php') {
      // eslint-disable-next-line global-require
      require('prismjs/components/prism-markup-templating.js');
    }
    // eslint-disable-next-line global-require, import/no-dynamic-require
    require(`prismjs/components/prism-${lang}`);
  });

  // --- Custom grammars (not yet in the bundled Prism release) ---
  // Loaded while globalThis.Prism is mounted, exactly like the component files
  // above. Remove this once `kdl` is available via `additionalLanguages`.
  // eslint-disable-next-line global-require
  require('@site/src/prism/prism-kdl');

  // Clean up and eventually restore former globalThis.Prism object (if any)
  delete (globalThis as Optional<typeof globalThis, 'Prism'>).Prism;
  if (typeof PrismBefore !== 'undefined') {
    globalThis.Prism = PrismObject;
  }
}
