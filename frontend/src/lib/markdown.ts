import MarkdownIt from 'markdown-it';
import taskLists from 'markdown-it-task-lists';
import DOMPurify from 'dompurify';
import hljs from 'highlight.js/lib/core';
import rust from 'highlight.js/lib/languages/rust';
import typescript from 'highlight.js/lib/languages/typescript';
import javascript from 'highlight.js/lib/languages/javascript';
import json from 'highlight.js/lib/languages/json';
import sql from 'highlight.js/lib/languages/sql';
import bash from 'highlight.js/lib/languages/bash';
import xml from 'highlight.js/lib/languages/xml';
import css from 'highlight.js/lib/languages/css';
import python from 'highlight.js/lib/languages/python';
for (const [name, language] of Object.entries({ rust, typescript, javascript, json, sql, bash, html: xml, xml, css, python })) hljs.registerLanguage(name, language);
const md = new MarkdownIt({ html: false, linkify: true, breaks: false, highlight: (code, language) => language && hljs.getLanguage(language) ? hljs.highlight(code, { language, ignoreIllegals: true }).value : '' }).use(taskLists);
const attachmentPattern = /^\/api\/v1\/attachments\/[0-9a-f-]{36}$/;
md.renderer.rules.image = (tokens, index) => {
  const token = tokens[index]; const source = token.attrGet('src') || ''; const alt = md.utils.escapeHtml(token.content);
  if (!attachmentPattern.test(source)) return `<span class="external-image">[External image: ${alt || 'upload this image to embed it'}]</span>`;
  return `<img src="${md.utils.escapeHtml(source)}" alt="${alt}" loading="lazy" />`;
};
const originalLink = md.renderer.rules.link_open || ((tokens, index, options, _env, self) => self.renderToken(tokens, index, options));
md.renderer.rules.link_open = (tokens, index, options, env, self) => { tokens[index].attrSet('rel', 'noopener noreferrer'); tokens[index].attrSet('target', '_blank'); return originalLink(tokens, index, options, env, self); };
export function renderMarkdown(source: string): string {
  return DOMPurify.sanitize(md.render(source), {
    ALLOWED_TAGS: ['p','br','hr','h1','h2','h3','h4','h5','h6','strong','em','s','ul','ol','li','blockquote','pre','code','span','a','img','table','thead','tbody','tr','th','td','input'],
    ALLOWED_ATTR: ['href','src','alt','title','class','target','rel','type','checked','disabled','loading','start'],
    ALLOW_DATA_ATTR: false,
  });
}

