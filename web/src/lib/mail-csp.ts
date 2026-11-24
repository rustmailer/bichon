// Content-Security-Policy for the email body iframe.
//
// The body arrives as a JSON string and is rendered with `srcDoc`, so there
// is no HTTP response to attach a CSP header to. The policy therefore travels
// inside the document as a meta tag. A meta policy can only tighten the one
// inherited from the embedding page, which is exactly what we want for
// attacker-controlled email HTML.

export const buildCsp = (allowRemote: boolean) => {
  const remote = allowRemote ? ' http: https:' : '';
  // `frame-ancestors` and `sandbox` are intentionally absent: browsers ignore
  // them when the policy is delivered through a meta tag. The iframe element
  // already carries the sandbox attribute; `default-src 'none'` blocks inline
  // scripts here.
  return [
    "default-src 'none'",
    `img-src data: blob:${remote}`,
    "style-src 'unsafe-inline'",
    'font-src data:',
    `media-src data: blob:${remote}`,
    "object-src 'none'",
    "base-uri 'none'",
    "form-action 'none'",
  ].join('; ');
};

export const withCsp = (html: string, allowRemote: boolean) => {
  const meta = `<meta http-equiv="Content-Security-Policy" content="${buildCsp(allowRemote)}">`;
  const headMatch = /<head(?:\s[^>]*)?>/i.exec(html);
  if (headMatch) {
    const end = headMatch.index + headMatch[0].length;
    return html.slice(0, end) + meta + html.slice(end);
  }
  const htmlMatch = /<html(?:\s[^>]*)?>/i.exec(html);
  if (htmlMatch) {
    const end = htmlMatch.index + htmlMatch[0].length;
    return `${html.slice(0, end)}<head>${meta}</head>${html.slice(end)}`;
  }
  const doctypeMatch = /<!doctype[^>]*>/i.exec(html);
  if (doctypeMatch) {
    const end = doctypeMatch.index + doctypeMatch[0].length;
    return html.slice(0, end) + meta + html.slice(end);
  }
  return meta + html;
};
