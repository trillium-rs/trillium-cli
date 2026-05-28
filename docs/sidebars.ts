import type {SidebarsConfig} from '@docusaurus/plugin-content-docs';

const sidebars: SidebarsConfig = {
  guideSidebar: [
    'welcome',
    'installing',
    'serve',
    'proxy',
    {
      type: 'category',
      label: 'gateway',
      link: {type: 'doc', id: 'gateway/overview'},
      items: [
        'gateway/overview',
        'gateway/routing',
        'gateway/rewrite-html',
        'gateway/virtual-hosts',
      ],
    },
    'client',
    'bench',
    'dev-server',
    'grpc',
  ],
};

export default sidebars;
