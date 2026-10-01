// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// A tiny hash router: #/, #/grid, #/item/<id>, #/backups, #/settings. No dependency.

import { parseRoute, type Route } from './routes.ts';

export type { Route };

class Router {
  route = $state.raw<Route>(typeof location === 'undefined' ? { name: 'home' } : parseRoute(location.hash));
  /** The route we came from, so "Back" can return to Backups or Settings' caller. */
  previous = $state.raw<Route | null>(null);

  start(): () => void {
    const onHash = () => {
      this.previous = this.route;
      this.route = parseRoute(location.hash);
    };
    window.addEventListener('hashchange', onHash);
    return () => window.removeEventListener('hashchange', onHash);
  }
}

export const router = new Router();

export function navigate(path: string): void {
  if (location.hash === `#${path}`) return;
  location.hash = path;
}

export function href(path: string): string {
  return `#${path}`;
}
