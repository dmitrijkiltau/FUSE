const addTocNavToSidebar = () => {
  const sidebar = document.querySelector('.sidebar');
  if (!sidebar) return;
  const toc = document.createElement('nav');
  toc.classList.add('toc');
  sidebar.appendChild(toc);

  const headers = document.querySelectorAll('.panel :is(h2, h3)');
  headers.forEach(header => {
    const id = header.id || header.textContent.trim().toLowerCase().replace(/\s+/g, '-');
    header.id = id;

    const link = document.createElement('a');
    link.href = `#${id}`;
    link.textContent = header.textContent;
    link.classList.add('toc-item');
    link.classList.add(`toc-${header.tagName.toLowerCase()}`);

    toc.appendChild(link);
  });
}

const updateTocActiveLink = () => {
  const tocLinks = document.querySelectorAll('.toc .toc-item');
  const headers = document.querySelectorAll('.panel :is(h2, h3)');

  let activeLink = null;
  for (const header of headers) {
    const rect = header.getBoundingClientRect();
    if (rect.top <= 100) {
      activeLink = document.querySelector(`.toc a[href="#${header.id}"]`);
    } else {
      break;
    }
  }

  tocLinks.forEach(link => link.classList.toggle('is-active', link === activeLink));
}

const generateToc = () => {
  addTocNavToSidebar();
  updateTocActiveLink();
  window.addEventListener('scroll', updateTocActiveLink);
}

export { generateToc };
