# Bundled app themes: sources and licences

The app themes in `src/lib/themes/bundled.ts` map published colour palettes onto Reacher CM's UI tokens and take each palette's terminal colours. The mapping is ours, and some colours were darkened or lightened so that text and white-on-accent buttons stay readable (see `scripts/app-themes-test.mjs`); the palettes belong to their authors, under these licences.

| Theme | Palette by | Licence | Source |
|---|---|---|---|
| Nord | Arctic Ice Studio and Sven Greb | MIT | https://github.com/nordtheme/nord |
| Solarized Dark, Solarized Light | Ethan Schoonover | MIT | https://github.com/altercation/solarized |
| Dracula | Dracula Theme (Zeno Rocha and contributors) | MIT | https://github.com/dracula/dracula-theme |
| Catppuccin Mocha, Catppuccin Latte | Catppuccin | MIT | https://github.com/catppuccin/palette |
| Gruvbox Dark, Gruvbox Light | Pavel Pertsev (morhetz) | MIT | https://github.com/morhetz/gruvbox |
| Tokyo Night, Tokyo Night Day | enkia; Folke Lemaitre for the terminal colours | MIT | https://github.com/tokyo-night/tokyo-night-vscode-theme, https://github.com/folke/tokyonight.nvim |
| One Dark, One Light | GitHub Inc. (Atom) | MIT | https://github.com/atom/atom/tree/master/packages/one-dark-syntax |
| Rosé Pine, Rosé Pine Dawn | Rosé Pine | MIT | https://github.com/rose-pine/palette |
| GitHub Dark, GitHub Light | GitHub Inc. (Primer) | MIT | https://github.com/primer/primitives |
| High Contrast Dark, High Contrast Light | Reacher CM | GPL-3.0-or-later (this project's licence) | — |
| Ocean, Forest, Sunset, Amethyst and Cherry (Dark and Light), Neon Night | Reacher CM | GPL-3.0-or-later (this project's licence) | `src/lib/themes/colorful.ts` |

Each of these projects asks that its copyright notice and the MIT permission notice go with copies of its work. The palettes here are colour values rather than code, but in that spirit: the copyright in each palette belongs to the people named above, and the MIT licence text is in this repository's `LICENSE-MIT`.
