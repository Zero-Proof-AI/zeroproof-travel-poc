FROM node:18

WORKDIR /app

COPY package*.json ./

# Fix git SSH → HTTPS for dependencies
RUN git config --global url."https://github.com/".insteadOf git@github.com: && \
    git config --global url."https://".insteadOf git://

# Install dependencies
RUN npm install 

# Copy source code
COPY . .

# Critical: Copy the pre-Downloaded ZK files required by @reclaimprotocol/zk-fetch (and its internal zk-symmetric-crypto)
# This ensures the SDK finds the necessary .wasm, .zkey, etc. files at the expected path for redactions
# The downloaded files are in node_modules/.ignored/@reclaimprotocol/zk-symmetric-crypto
# Then copy these files to the local lib/ directory in the container, and from there to the expected path
#  $node node_modules/@reclaimprotocol/zk-fetch/scripts/download-files.js
#  cp -r $(pwd)/node_modules/.ignored/@reclaimprotocol/zk-symmetric-crypto ./lib/@reclaimprotocol

RUN cp -r ./lib/@reclaimprotocol/zk-symmetric-crypto/resources ./node_modules/@reclaimprotocol/zk-symmetric-crypto/

# Use built-in non-root node user
USER node

EXPOSE 8003

CMD ["node", "index.js"]