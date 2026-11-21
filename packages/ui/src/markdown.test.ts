import { describe, expect, it } from 'vitest';
import { plainText, renderMarkdown } from './markdown';

describe('markdown', () => {
  it('escapes HTML from the model', () => {
    const html = renderMarkdown('<img src=x onerror=alert(1)> **hi**');
    expect(html).not.toContain('<img');
    expect(html).toContain('&lt;img');
    expect(html).toContain('<strong>hi</strong>');
  });

  it('renders lists, paragraphs and code', () => {
    const html = renderMarkdown('قیمت‌ها:\n- دلار: `۸۵,۰۰۰`\n- طلا\n\n1. one\n2. two\nend');
    expect(html).toBe(
      '<p>قیمت‌ها:</p><ul><li>دلار: <code>۸۵,۰۰۰</code></li><li>طلا</li></ul><ol><li>one</li><li>two</li></ol><p>end</p>'
    );
  });

  it('keeps line breaks inside a paragraph and handles unfinished marks while typing', () => {
    expect(renderMarkdown('a\nb')).toBe('<p>a<br>b</p>');
    expect(renderMarkdown('**unfinished')).toBe('<p>**unfinished</p>');
    expect(renderMarkdown('*it* and **b**')).toBe('<p><em>it</em> and <strong>b</strong></p>');
  });

  it('gives plain text for one-line places', () => {
    expect(plainText('## Title\n**bold** `x`\n- item')).toBe('Title\nbold x\n• item');
  });
});
