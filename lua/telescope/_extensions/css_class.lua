-- :Telescope css_class            (undefined classes, the default)
-- :Telescope css_class undefined
-- :Telescope css_class classes
local has_telescope, telescope = pcall(require, "telescope")
if not has_telescope then
	error("css_class extension requires nvim-telescope/telescope.nvim")
end

local pickers = require("telescope.pickers")
local finders = require("telescope.finders")
local make_entry = require("telescope.make_entry")
local conf = require("telescope.config").values
local css = require("css-class-lsp")

local function qf_picker(title, items, opts)
	pickers
		.new(opts, {
			prompt_title = title,
			finder = finders.new_table({
				results = items,
				entry_maker = opts.entry_maker or make_entry.gen_from_quickfix(opts),
			}),
			previewer = conf.qflist_previewer(opts),
			sorter = conf.generic_sorter(opts),
		})
		:find()
end

local function undefined(opts)
	opts = opts or {}
	css.undefined_items(function(items)
		if #items == 0 then
			vim.notify("css-class-lsp: no undefined classes")
			return
		end
		-- Show "file:line  class" rather than the full message.
		for _, it in ipairs(items) do
			it.text = it.user_data.class
		end
		qf_picker("Undefined CSS classes", items, opts)
	end)
end

local function classes(opts)
	opts = opts or {}
	css.class_items(function(items)
		qf_picker("CSS classes", items, opts)
	end)
end

return telescope.register_extension({
	exports = {
		css_class = undefined,
		undefined = undefined,
		classes = classes,
	},
})
