// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

// https://astro.build/config
export default defineConfig({
	integrations: [
		starlight({
			title: 'Fuse Docs',
			sidebar: [
				{
					label: 'Guides',
					items: [
						{ label: 'Language tour', slug: 'guides/getting-started' },
						{ label: 'Use cases', slug: 'guides/running-examples' },
					],
				},
				{
					label: 'Reference',
					items: [
						{ label: 'Language', slug: 'reference/language' },
						{ label: 'Runtime', slug: 'reference/runtime' },
						{ label: 'CLI', slug: 'reference/cli' },
					],
				},
			],
		}),
	],
});
