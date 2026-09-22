import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { AdminPage } from './pages/AdminPage';
import { HomePage } from './pages/HomePage';
import './styles.css';

const isAdmin = window.location.pathname === '/admin' || window.location.pathname.startsWith('/admin/');

createRoot(document.getElementById('root')!).render(
  <StrictMode>{isAdmin ? <AdminPage /> : <HomePage />}</StrictMode>
);
