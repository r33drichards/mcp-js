// Lazy-load <video data-src> elements: the media is only fetched once the
// player scrolls near the viewport, keeping initial page loads light.
document.addEventListener("DOMContentLoaded", function () {
  var videos = Array.prototype.slice.call(
    document.querySelectorAll("video[data-src]")
  );
  if (!videos.length) {
    return;
  }

  function load(video) {
    if (video.dataset.loaded) {
      return;
    }
    var sources = video.querySelectorAll("source[data-src]");
    for (var i = 0; i < sources.length; i++) {
      sources[i].src = sources[i].getAttribute("data-src");
      sources[i].removeAttribute("data-src");
    }
    if (video.getAttribute("data-src")) {
      video.src = video.getAttribute("data-src");
      video.removeAttribute("data-src");
    }
    video.load();
    video.dataset.loaded = "true";
  }

  // Fallback for browsers without IntersectionObserver: load eagerly.
  if (!("IntersectionObserver" in window)) {
    videos.forEach(load);
    return;
  }

  var observer = new IntersectionObserver(
    function (entries) {
      entries.forEach(function (entry) {
        if (entry.isIntersecting) {
          load(entry.target);
          observer.unobserve(entry.target);
        }
      });
    },
    { rootMargin: "200px 0px" }
  );

  videos.forEach(function (video) {
    observer.observe(video);
  });
});
