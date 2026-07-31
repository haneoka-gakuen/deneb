export async function createDenebRuntime({ abiVersion, signal }) {
  if (abiVersion !== 1) throw new Error(`fixture only supports ABI 1, received ${abiVersion}`);
  let disposed = false;
  return {
    abiVersion: 1,
    async mount({ root, projectUrl }) {
      const response = await fetch(projectUrl, { signal });
      if (!response.ok) throw new Error(`project request failed (${response.status})`);
      const project = await response.json();
      const scene = project.scenes?.[project.entryScene];
      const line = scene?.commands?.find((command) => typeof command.text === "string");
      root.replaceChildren();
      const view = document.createElement("article");
      view.className = "deneb-contract-fixture";
      const title = document.createElement("h1");
      title.textContent =
        typeof project.title === "string" ? project.title : project.title?.en || project.id;
      const text = document.createElement("p");
      text.textContent = line?.text || "Runtime ABI 1 mounted successfully.";
      view.append(title, text);
      root.append(view);
      return {
        async dispose() {
          view.remove();
        },
      };
    },
    async dispose() {
      disposed = true;
      void disposed;
    },
  };
}
