import { defineConfig } from 'astro/config';
import preact from '@astrojs/preact';
import starlight from '@astrojs/starlight';

export default defineConfig({
  site: 'https://jagreehal.github.io',
  base: '/autotel-rust',
  integrations: [
    preact({ devtools: false }),
    starlight({
      title: 'autotel',
      // `404.md` is the only /404 route, through Starlight's `[...slug]`.
      disable404Route: true,
      favicon: '/favicon.svg',
      components: {
        PageTitle: './src/components/PageTitle.astro',
        ThemeProvider: './src/components/ThemeProvider.astro',
        ThemeSelect: './src/components/ThemeSelect.astro',
        Hero: './src/components/SplashHero.astro',
      },
      social: [
        {
          icon: 'github',
          label: 'GitHub',
          href: 'https://github.com/jagreehal/autotel-rust',
        },
      ],
      customCss: [
        '@fontsource-variable/inter',
        '@fontsource/jetbrains-mono',
        '@fontsource/jetbrains-mono/500.css',
        '@fontsource/jetbrains-mono/600.css',
        '@fontsource/jetbrains-mono/700.css',
        './src/styles/custom.css',
      ],
      sidebar: [
        {
          label: 'Getting Started',
          items: [
            { label: 'Introduction', slug: 'introduction' },
            { label: 'Quick Start', slug: 'quick-start' },
            { label: 'Configuration', slug: 'configuration' },
          ],
        },
        {
          label: 'Servers',
          items: [
            { label: 'Axum', slug: 'servers/axum' },
            { label: 'gRPC', slug: 'servers/grpc' },
          ],
        },
        {
          label: 'Guides',
          items: [
            { label: 'What You Can Record', slug: 'recording' },
            { label: 'Log Files', slug: 'log-files' },
            { label: 'Testing', slug: 'testing' },
            { label: 'Workshop', slug: 'workshop' },
            { label: 'Agent Skill', slug: 'agent-skill' },
          ],
        },
      ],
    }),
  ],
});
