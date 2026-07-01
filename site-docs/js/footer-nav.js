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

  function navTitleFor(link) {
    var href = link.getAttribute("href");
    if (!href) {
      return "";
    }

    var candidates = document.querySelectorAll(
      '.dropdown-item[href="' + href + '"], .nav-link[href="' + href + '"]'
    );

    for (var i = 0; i < candidates.length; i++) {
      var text = candidates[i].textContent.trim();
      if (text) {
        return text;
      }
    }

    return "";
  }

  function appendItem(link, kind) {
    if (!link) {
      return;
    }
    var item = document.createElement("li");
    item.className = "docs-footer-nav-item docs-footer-nav-item-" + kind;

    var clone = link.cloneNode(true);
    clone.classList.add("docs-footer-nav-link");
    clone.innerHTML = "";

    var label = navTitleFor(link);
    if (kind === "prev") {
      var prevIcon = document.createElement("i");
      prevIcon.className = "fa fa-arrow-left";
      prevIcon.setAttribute("aria-hidden", "true");
      clone.appendChild(prevIcon);
    }

    clone.appendChild(document.createTextNode(label || link.textContent.trim()));

    if (kind === "next") {
      var nextIcon = document.createElement("i");
      nextIcon.className = "fa fa-arrow-right";
      nextIcon.setAttribute("aria-hidden", "true");
      clone.appendChild(document.createTextNode(" "));
      clone.appendChild(nextIcon);
    } else {
      clone.insertBefore(document.createTextNode(" "), clone.childNodes[1] || null);
    }

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
