import '@mantine/core/styles.css';

import { MantineProvider } from '@mantine/core';
import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { Shell } from './Shell';

const root = document.getElementById('root');
if (!root) throw new Error('Missing #root element');

createRoot(root).render(
  <StrictMode>
    <MantineProvider defaultColorScheme="auto">
      <Shell />
    </MantineProvider>
  </StrictMode>,
);
