const rspack = require("@rspack/core");

/** @type {import("@rspack/core").Configuration} */
module.exports = {
	target: "node",
	entry: "./src/index.js",
	output: {
		filename: "[name].js"
	},
	experiments: {
		css: false
	},
	performance: false,
	module: {
		rules: [
			{
				test: /\.css$/,
				use: [rspack.CssExtractRspackPlugin.loader, "css-loader"]
			}
		]
	},
	plugins: [new rspack.CssExtractRspackPlugin()],
	optimization: {
		chunkIds: "named",
		moduleIds: "named",
		splitChunks: {
			chunks: "all",
			cacheGroups: {
				fragment: {
					minChunks: 1,
					maxSize: 200 * 1024,
					priority: 10
				}
			}
		}
	}
};
