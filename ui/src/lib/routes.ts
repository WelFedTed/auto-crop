// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// Route parsing for the hash router (pure, so it can be unit tested).

export type Route =
  | { name: 'home' }
  | { name: 'grid' }
  | { name: 'item'; id: number }
  | { name: 'backups' }
  | { name: 'settings' };

export function parseRoute(hash: string): Route {
  const path = hash.replace(/^#/, '').replace(/^\/+/, '').replace(/\/+$/, '');
  const parts = path.split('/');
  switch (parts[0]) {
    case 'grid':
      return { name: 'grid' };
    case 'backups':
      return { name: 'backups' };
    case 'settings':
      return { name: 'settings' };
    case 'item': {
      const id = Number(parts[1]);
      return Number.isInteger(id) ? { name: 'item', id } : { name: 'grid' };
    }
    default:
      return { name: 'home' };
  }
}

export function routePath(route: Route): string {
  switch (route.name) {
    case 'home':
      return '/';
    case 'item':
      return `/item/${route.id}`;
    default:
      return `/${route.name}`;
  }
}
