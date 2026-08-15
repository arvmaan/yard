import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import '@fontsource-variable/space-grotesk'
import '@fontsource-variable/ibm-plex-sans'
import '@fontsource/ibm-plex-mono/500.css'
import '@xyflow/react/dist/style.css'
import './index.css'
import App from './App.tsx'
import { applyTheme, readTheme } from './theme'

applyTheme(readTheme())

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
