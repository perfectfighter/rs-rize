import React from 'react';
import './native-bridge.js';
import '@fontsource/lato/latin-300.css';
import '@fontsource/lato/latin-400.css';
import '@fontsource/lato/latin-700.css';
import {createRoot} from 'react-dom/client';
import App from './App.jsx';
import './styles.css';
createRoot(document.getElementById('root')).render(<React.StrictMode><App/></React.StrictMode>);
