document.addEventListener("DOMContentLoaded", function () {
  var footer = document.querySelector("footer.col-md-12");
  var prevLink = document.querySelector('a[rel="prev"]');
  var nextLink = document.querySelector('a[rel="next"]');

  // Replace the default MkDocs footer text with the project GitHub link.
  var footerText = footer ? footer.querySelector("p") : null;
  if (footerText) {
    footerText.innerHTML =
      '<a href="https://github.com/r33drichards/mcp-js">GitHub repository</a>';
  }

  if (!footer || (!prevLink && !nextLink)) {
    return;
  }

  var prevItem = prevLink ? prevLink.closest("li") : null;
  var nextItem = nextLink ? nextLink.closest("li") : null;

  if (!prevItem && !nextItem) {
    return;
  }

  var nav = document.createElement("nav");
  nav.className = "docs-footer-nav";
  nav.setAttribute("aria-label", "Page navigation");

  var list = document.createElement("ul");
  list.className = "docs-footer-nav-list";

  if (prevItem) {
    list.appendChild(prevItem);
  }
  if (nextItem) {
    list.appendChild(nextItem);
  }

  nav.appendChild(list);

  var hr = footer.querySelector("hr");
  var marker = footer.querySelector("p");

  if (marker) {
    footer.insertBefore(nav, marker);
  } else if (hr && hr.nextSibling) {
    footer.insertBefore(nav, hr.nextSibling);
  } else {
    footer.appendChild(nav);
  }
});
