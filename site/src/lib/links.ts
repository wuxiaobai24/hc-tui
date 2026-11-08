export function relativePrefix(pathname: string): string {
  const clean = pathname.endsWith('/index.html')
    ? pathname.slice(0, -'index.html'.length)
    : pathname;
  const depth = clean.split('/').filter(Boolean).length;
  return depth === 0 ? './' : '../'.repeat(depth);
}

export function href(pathname: string, target = ''): string {
  const clean = target.replace(/^\//, '');
  const file = clean
    ? clean.endsWith('/')
      ? `${clean}index.html`
      : clean
    : 'index.html';
  return `${relativePrefix(pathname)}${file}`;
}
