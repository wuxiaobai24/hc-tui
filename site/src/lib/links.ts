// 站内链接一律是相对路径：从当前页面的深度推出 `./` 或 `../..`。
// 部署到 GitHub Pages 的项目子路径时，pathname 会带上 base（如 `/hc-tui/docs/x/`），
// 多出来的这一段会让深度算多一层，所以先剥掉 base 再算。
const BASE = (import.meta.env.BASE_URL || '/').replace(/\/+$/, '');

export function relativePrefix(pathname: string): string {
  let clean = pathname.endsWith('/index.html')
    ? pathname.slice(0, -'index.html'.length)
    : pathname;
  if (BASE && clean.startsWith(BASE)) {
    clean = clean.slice(BASE.length);
    if (!clean.startsWith('/')) clean = `/${clean}`;
  }
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
