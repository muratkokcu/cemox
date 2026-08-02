FROM node:24-bookworm-slim

ENV NODE_ENV=production
WORKDIR /app

COPY package.json package-lock.json ./
RUN npm ci

COPY . .
RUN npm run build && npm prune --omit=dev
RUN mkdir -p /app/data && chown -R node:node /app

USER node
EXPOSE 4100
CMD ["node", "server.js"]
