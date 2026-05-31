document.addEventListener("DOMContentLoaded", function () {
  var footer = document.querySelector("footer.col-md-12");
  if (!footer) {
    return;
  }

  var prevLink = document.querySelector('a[rel="prev"]');
  var nextLink = document.querySelector('a[rel="next"]');
  var footerText = footer.querySelector("p");

  if (footerText) {
    footerText.remove();
  }

  if (!prevLink && !nextLink) {
    return;
  }

  var nav = document.createElement("nav");
  nav.className = "docs-footer-nav";
  nav.setAttribute("aria-label", "Page navigation");

  var list = document.createElement("ul");
  list.className = "docs-footer-nav-list";

  function appendItem(link, kind) {
    if (!link) {
      return;
    }
    var item = document.createElement("li");
    item.className = "docs-footer-nav-item docs-footer-nav-item-" + kind;

    var clone = link.cloneNode(true);
    clone.classList.add("docs-footer-nav-link");
    item.appendChild(clone);
    list.appendChild(item);
  }

  appendItem(prevLink, "prev");
  appendItem(nextLink, "next");
  nav.appendChild(list);

  var hr = footer.querySelector("hr");
  if (hr && hr.nextSibling) {
    footer.insertBefore(nav, hr.nextSibling);
  } else {
    footer.appendChild(nav);
  }
});
