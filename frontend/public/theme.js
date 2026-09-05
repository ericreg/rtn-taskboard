try {
  const theme = localStorage.getItem('taskboard-theme') || 'system';
  document.documentElement.dataset.theme = theme === 'system' ? (matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light') : theme;
} catch { document.documentElement.dataset.theme = 'light'; }
