import { flushSync } from 'react-dom';
import { createRoot } from 'react-dom/client';
import { describe, expect, it } from 'vitest';

import { App } from './App';

describe('Admin shell', () => {
  it('explains the separate data and control planes', () => {
    const container = document.createElement('div');
    const root = createRoot(container);

    flushSync(() => root.render(<App />));

    expect(container.textContent).toContain('Data plane');
    expect(container.textContent).toContain('Control plane');
    expect(container.textContent).toContain('separate from the public MCP');

    root.unmount();
  });
});
